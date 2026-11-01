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
import java.io.File

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
    private var seenErasures = 0

    private var readable = false

    private fun preferences(): SharedPreferences = getSharedPreferences(Settings.FILE, MODE_PRIVATE)

    private fun personalFile(name: String) = File(filesDir, name)

    /** The sealed trace. It only exists in a build holding an owner's key. */
    private fun traceFile(): File? = preferences()
        .takeIf { Settings.hasTraceKey(this) && it.getBoolean(Settings.TRACES, true) }
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
        seenErasures = saved.getInt(Settings.ERASURES, 0)
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
            val sealed = personalFile(Settings.MEMORY_FILE)
            keyboard.setMemoryKey(personalFile(Settings.MEMORY_PUBLIC_FILE).readBytes())
            keyboard.loadMemory(if (sealed.exists()) sealed.readBytes() else ByteArray(0), secret)
            secret.fill(0)
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
        send(Event.HeightScale(saved.getInt(Settings.HEIGHT, 100) / 100f))
        val erasures = saved.getInt(Settings.ERASURES, 0)
        if (erasures != seenErasures) {
            seenErasures = erasures
            send(Event.ForgetAll)
        }
    }

    private fun writeFile(target: File, bytes: ByteArray) {
        val temporary = File(filesDir, "${target.name}.tmp")
        temporary.writeBytes(bytes)
        temporary.renameTo(target)
    }

    /** The memory arrives sealed to a public key, so this works on a locked phone. */
    private fun saveMemory(sealed: ByteArray) {
        Core.onIo { writeFile(personalFile(Settings.MEMORY_FILE), sealed) }
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
        val height = ViewGroup.LayoutParams.WRAP_CONTENT
        root.addView(keyboardView(), FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, height))
        val inline = InlineStrip(this)
        root.addView(inline, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, height))
        strip = inline
        return root
    }

    override fun onCreateInlineSuggestionsRequest(uiExtras: Bundle): InlineSuggestionsRequest =
        InlineStrip.request(this)

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

    override fun onStartInputView(info: EditorInfo, restarting: Boolean) {
        super.onStartInputView(info, restarting)
        guarded {
            applySettings()
            val field = Field(info.inputType.toUInt(), info.imeOptions.toUInt())
            readable = keyboard.mayRead(field)
            protectScreen(!readable || !preferences().getBoolean(Settings.SCREENSHOTS, false))
            val before = if (readable) textBeforeCursor() else ""
            val after = if (readable) textAfterCursor() else ""
            send(Event.Shown(field, before, after, info.initialSelEnd.coerceAtLeast(0).toUInt()))
            send(Event.Session((System.currentTimeMillis() / 1000).toULong()))
            loadMemory()
        }
    }

    /** Keeps the keyboard, whose bar shows what is typed, out of screenshots and recordings. */
    private fun protectScreen(on: Boolean) {
        val flag = WindowManager.LayoutParams.FLAG_SECURE
        window?.window?.run { if (on) addFlags(flag) else clearFlags(flag) }
    }

    override fun onFinishInputView(finishingInput: Boolean) {
        strip?.clear()
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
            is Command.Enter -> enter()
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
            is Command.SaveMemory -> saveMemory(command.sealed)
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

    private fun enter() {
        val info = currentInputEditorInfo ?: return
        val action = info.imeOptions and EditorInfo.IME_MASK_ACTION
        val noAction = info.imeOptions and EditorInfo.IME_FLAG_NO_ENTER_ACTION != 0
        if (noAction || action == EditorInfo.IME_ACTION_NONE || action == EditorInfo.IME_ACTION_UNSPECIFIED) {
            sendDownUpKeyEvents(KeyEvent.KEYCODE_ENTER)
        } else {
            currentInputConnection?.performEditorAction(action)
        }
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
        const val KEY_BYTES = 32
    }
}
