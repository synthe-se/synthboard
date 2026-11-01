package app.synthboard

import android.content.SharedPreferences
import android.graphics.Paint
import android.inputmethodservice.InputMethodService
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.HapticFeedbackConstants
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InlineSuggestionsRequest
import android.view.inputmethod.InlineSuggestionsResponse
import android.view.inputmethod.InputMethodManager
import android.widget.FrameLayout
import app.synthboard.core.Command
import app.synthboard.core.EmojiItem
import app.synthboard.core.Event
import app.synthboard.core.Field
import app.synthboard.core.Haptic
import app.synthboard.core.Icon
import app.synthboard.core.KEY_BYTES
import java.io.File
import java.io.FileOutputStream

/**
 * Forwards Android events to the Rust core and applies its commands. The core
 * makes every decision, including when the single deadline is due. Nothing that
 * fails here may stop typing, so every way in goes through `send` or `Core`.
 */
class SynthboardService : InputMethodService() {
    private val keyboard = Core.keyboard
    private val vault = Vault.ofDevice()
    private val handler = Handler(Looper.getMainLooper())
    private val deadline = Runnable { send(Event.Deadline(SystemClock.uptimeMillis().toULong())) }
    private var previewRunning = false
    private var previewStale = false
    private var view: KeyboardView? = null
    private var strip: InlineStrip? = null
    private var emoji: List<Pair<String, List<EmojiItem>>> = emptyList()
    private var emojiLoading = false
    private var tone = 0
    private var haptics = true

    private var readable = false
    private val hasTraceKey by lazy { Settings.hasTraceKey(this) }

    private fun preferences(): SharedPreferences = getSharedPreferences(Settings.FILE, MODE_PRIVATE)

    private fun personalFile(name: String) = File(filesDir, name)

    /** The sealed trace. It only exists in a build holding an owner's key. */
    private fun traceFile(): File? = preferences()
        .takeIf { hasTraceKey && it.getBoolean(Settings.TRACES, true) }
        ?.let { File(filesDir, TraceProvider.FILE) }

    private fun readAsset(name: String): ByteArray = assets.open(name).use { it.readBytes() }

    /** Loads what the core needs, once per process. */
    private fun loadLanguageData() {
        if (Core.loading.getAndSet(true)) return
        Core.onIo {
            Settings.traceKey(this)?.let(keyboard::setTraceKey)
            keyboard.loadLexicon(readAsset("lexicon.csv"))
            keyboard.loadLanguage(readAsset("affinity.bin"), readAsset("forms.tsv"), readAsset("next.tsv"))
            handler.post { send(Event.LanguageReady) }
        }
    }

    /** Reads the user's settings, once per process. */
    private fun restoreUserData() {
        val saved = preferences()
        tone = saved.getInt(Settings.TONE, 0)
        val recents = saved.getString(Settings.RECENTS, "").orEmpty().split(' ').filter(String::isNotEmpty)
        send(Event.Recents(recents))
    }

    /**
     * Brings memory back for this opening: it leaves RAM whenever the keyboard
     * hides. Only Rust sees it in the clear. A locked phone refuses the secret
     * key, and the keyboard then works without memory.
     */
    private fun loadMemory() {
        Core.onIo {
            val secret = memorySecret()
            try {
                keyboard.setMemoryKey(personalFile(Settings.MEMORY_PUBLIC_FILE).readBytes())
                val sealed = personalFile(Settings.MEMORY_FILE)
                val saved = if (sealed.exists()) sealed.readBytes() else null
                if (!keyboard.loadMemory(saved, secret)) {
                    // Left in place it would stop every later save, silently.
                    sealed.renameTo(personalFile(Settings.DAMAGED_MEMORY_FILE))
                    keyboard.loadMemory(null, secret)
                }
            } finally {
                secret.fill(0)
            }
        }
    }

    /** The secret key of memory, made with its public half on first use. */
    private fun memorySecret(): ByteArray {
        val sealed = personalFile(Settings.MEMORY_SECRET_FILE)
        if (sealed.exists()) return vault.open(sealed.readBytes())
        val keys = keyboard.newMemoryKeys()
        val secret = keys.copyOfRange(0, KEY_BYTES)
        writeFile(personalFile(Settings.MEMORY_PUBLIC_FILE), keys.copyOfRange(KEY_BYTES, keys.size))
        writeFile(sealed, vault.seal(secret))
        keys.fill(0)
        return secret
    }

    /** Loads the emoji the first time the panel opens: most sessions never do. */
    private fun loadEmoji() {
        if (emojiLoading) return
        emojiLoading = true
        Core.onIo {
            val paint = Paint()
            val groups = keyboard.loadEmoji(readAsset("emoji.tsv")).map { group ->
                group.name to group.items.filter { paint.hasGlyph(it.emoji) }
            }
            handler.post {
                emoji = groups
                guarded { view?.setEmoji(groups) }
            }
        }
    }

    /**
     * Back-pressure, with no timer: one preview runs at a time, and when it
     * ends another starts only if the finger moved meanwhile.
     */
    private fun requestPreview() {
        if (previewRunning) {
            previewStale = true
            return
        }
        previewRunning = true
        Core.onPreview {
            val word = keyboard.preview()
            handler.post {
                previewRunning = false
                guarded { view?.setPreview(word) }
                if (previewStale) {
                    previewStale = false
                    requestPreview()
                }
            }
        }
    }

    /** Reads what the settings screen stored. Nothing here polls: it runs when the keyboard opens. */
    private fun applySettings() {
        val saved = preferences()
        haptics = saved.getBoolean(Settings.HAPTICS, true)
        view?.showTrail = saved.getBoolean(Settings.TRAIL, true)
        send(Event.HeightScale(saved.getInt(Settings.HEIGHT, Settings.FULL_HEIGHT) / Settings.FULL_HEIGHT.toFloat()))
        send(Event.Alphabet(saved.getBoolean(Settings.QWERTY, false)))
        send(Event.Screenshots(saved.getBoolean(Settings.SCREENSHOTS, false)))
    }

    private fun writeFile(target: File, bytes: ByteArray) {
        val temporary = File(filesDir, "${target.name}.tmp")
        FileOutputStream(temporary).use { out ->
            out.write(bytes)
            // Without this a power cut can keep the rename and lose the bytes.
            out.fd.sync()
        }
        check(temporary.renameTo(target))
    }

    /** Rust seals to a public key, so this works on a locked phone. */
    private fun saveMemory() {
        Core.onIo {
            val sealed = keyboard.sealMemory()
            if (sealed.isNotEmpty()) writeFile(personalFile(Settings.MEMORY_FILE), sealed)
        }
    }

    private fun appendTrace(record: String) {
        val target = traceFile() ?: return
        Core.onIo {
            if (target.length() < TraceProvider.MAX_BYTES) target.appendText(record + "\n")
        }
    }

    override fun onCreate() {
        super.onCreate()
        guarded { loadLanguageData() }
        guarded { restoreUserData() }
    }

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        view = null
        super.onDestroy()
    }

    override fun onCreateInputView(): View {
        val root = FrameLayout(this)
        guarded {
            val height = ViewGroup.LayoutParams.WRAP_CONTENT
            root.addView(keyboardView(), FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, height))
            val inline = InlineStrip(this)
            root.addView(inline, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, height))
            strip = inline
        }
        return root
    }

    override fun onCreateInlineSuggestionsRequest(uiExtras: Bundle): InlineSuggestionsRequest? {
        var request: InlineSuggestionsRequest? = null
        guarded { request = InlineStrip.request(this) }
        return request
    }

    override fun onInlineSuggestionsResponse(response: InlineSuggestionsResponse): Boolean {
        guarded { strip?.inflate(response.inlineSuggestions) }
        return true
    }

    private fun keyboardView() = KeyboardView(this, ::send).also {
        view = it
        it.onGestureMoved = ::requestPreview
        it.setEmoji(emoji)
        it.onToneChosen = { chosen ->
            tone = chosen
            preferences().edit().putInt(Settings.TONE, chosen).apply()
        }
    }

    override fun onEvaluateFullscreenMode(): Boolean = false

    /** Focus can move while hidden: nothing is readable until a field is shown. */
    override fun onStartInput(attribute: EditorInfo, restarting: Boolean) {
        super.onStartInput(attribute, restarting)
        readable = false
    }

    override fun onStartInputView(info: EditorInfo, restarting: Boolean) {
        super.onStartInputView(info, restarting)
        guarded {
            applySettings()
            val field = Field(info.inputType.toUInt(), info.imeOptions.toUInt())
            readable = keyboard.mayRead(field)
            val before = if (readable) textBeforeCursor() else ""
            val after = if (readable) textAfterCursor() else ""
            send(Event.Shown(field, before, after, info.initialSelEnd.coerceAtLeast(0).toUInt()))
            send(Event.Session((System.currentTimeMillis() / MILLIS_PER_SECOND).toULong()))
            loadMemory()
        }
    }

    /** Keeps the keyboard, whose bar shows what is typed, out of screenshots and recordings. */
    private fun protectScreen(on: Boolean) {
        val flag = WindowManager.LayoutParams.FLAG_SECURE
        window?.window?.run { if (on) addFlags(flag) else clearFlags(flag) }
    }

    override fun onFinishInputView(finishingInput: Boolean) {
        readable = false
        guarded { strip?.clear() }
        send(Event.Hidden)
        super.onFinishInputView(finishingInput)
    }

    override fun onUpdateSelection(oldStart: Int, oldEnd: Int, newStart: Int, newEnd: Int, composeStart: Int, composeEnd: Int) {
        super.onUpdateSelection(oldStart, oldEnd, newStart, newEnd, composeStart, composeEnd)
        send(Event.Selection(newStart.coerceAtLeast(0).toUInt(), newEnd.coerceAtLeast(0).toUInt()))
    }

    private fun textBeforeCursor(): String =
        currentInputConnection?.getTextBeforeCursor(CONTEXT_CHARS, 0)?.takeLast(CONTEXT_CHARS)?.toString() ?: ""

    private fun textAfterCursor(): String =
        currentInputConnection?.getTextAfterCursor(CONTEXT_CHARS, 0)?.take(CONTEXT_CHARS)?.toString() ?: ""

    private fun send(event: Event) {
        guarded {
            val step = keyboard.step(event)
            handler.removeCallbacks(deadline)
            step.deadline?.let { handler.postAtTime(deadline, it.toLong()) }

            val connection = currentInputConnection
            var wantsContext = false
            connection?.beginBatchEdit()
            for (command in step.commands) {
                // One failed command must not lose the ones after it.
                guarded {
                    if (command is Command.RequestContext) wantsContext = true else apply(command)
                }
            }
            connection?.endBatchEdit()
            if (wantsContext && readable) send(Event.Context(textBeforeCursor(), textAfterCursor()))
        }
    }

    private fun apply(command: Command) {
        val connection = currentInputConnection
        when (command) {
            is Command.Commit -> connection?.commitText(command.text, 1)
            is Command.DeleteBefore -> connection?.deleteSurroundingText(command.units.toInt(), 0)
            is Command.Select -> connection?.setSelection(command.start.toInt(), command.end.toInt())
            is Command.DeleteSelection -> connection?.commitText("", 1)
            is Command.NewLine -> sendDownUpKeyEvents(KeyEvent.KEYCODE_ENTER)
            is Command.EditorAction -> connection?.performEditorAction(command.action.toInt())
            is Command.ProtectScreen -> protectScreen(command.on)
            is Command.Picker -> getSystemService(InputMethodManager::class.java).showInputMethodPicker()
            is Command.Vibrate -> vibrate(command.kind)
            is Command.Keys -> {
                view?.setKeys(command.height, command.barHeight, command.keys)
                strip?.setHeight(command.barHeight.toInt())
            }
            is Command.Look -> view?.setLook(command.pressed, command.shift.toInt())
            is Command.Popup -> view?.setPopup(command)
            is Command.ForgetBubble -> view?.setForgetBubble(command)
            is Command.Suggestions -> view?.setSuggestions(command)
            is Command.UndoBar -> view?.setUndo(command.offered)
            is Command.RequestContext -> Unit
            is Command.Trail -> view?.setTrail(command.on)
            is Command.EmojiPanel -> {
                if (command.on) loadEmoji()
                view?.setPanel(command.on, command.recents, tone)
                strip?.setCovered(command.on)
            }
            is Command.EmojiSearch -> view?.setSearch(command.on, command.query, command.results)
            is Command.SaveMemory -> saveMemory()
            is Command.Trace -> appendTrace(command.record)
            is Command.SaveRecents ->
                preferences().edit().putString(Settings.RECENTS, command.items.joinToString(" ")).apply()
            is Command.Announce -> view?.contentDescription = spoken(command)
        }
    }

    private fun spoken(command: Command.Announce): String = when (command.icon) {
        Icon.SHIFT -> getString(R.string.key_shift)
        Icon.DELETE -> getString(R.string.key_delete)
        Icon.ENTER -> getString(R.string.key_enter)
        Icon.EMOJI -> getString(R.string.key_emoji)
        Icon.NONE -> command.text.ifEmpty { getString(R.string.key_space) }
    }

    private fun vibrate(kind: Haptic) {
        if (!haptics) return
        view?.performHapticFeedback(
            when (kind) {
                Haptic.TAP -> HapticFeedbackConstants.KEYBOARD_TAP
                Haptic.GESTURE_START -> HapticFeedbackConstants.GESTURE_START
                Haptic.LONG_PRESS -> HapticFeedbackConstants.LONG_PRESS
                Haptic.STEP -> HapticFeedbackConstants.TEXT_HANDLE_MOVE
            },
        )
    }

    private companion object {
        /** What the keyboard reads around the cursor: little, as it cannot be wiped from the JVM. */
        const val CONTEXT_CHARS = 256
        const val MILLIS_PER_SECOND = 1000
    }
}
