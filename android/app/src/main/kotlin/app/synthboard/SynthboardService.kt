package app.synthboard

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.SharedPreferences
import android.graphics.Paint
import android.inputmethodservice.InputMethodService
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.os.UserManager
import android.view.HapticFeedbackConstants
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
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
    private val handler = Handler(Looper.getMainLooper())
    private val deadline = Runnable { send(Event.Deadline(SystemClock.uptimeMillis().toULong())) }
    private var previewRunning = false
    private var previewStale = false
    private var view: KeyboardView? = null
    private var emoji: List<Pair<String, List<EmojiItem>>> = emptyList()
    private var emojiLoading = false
    private var tone = 0
    private var haptics = true
    private var seenErasures = 0

    /** Fires once after a reboot, when the user's own files become readable. */
    private val unlocked = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            guarded {
                restoreUserData()
                unregisterUnlockWatch()
            }
        }
    }
    private var watchingUnlock = false

    private fun isUnlocked(): Boolean = getSystemService(UserManager::class.java).isUserUnlocked

    private fun unregisterUnlockWatch() {
        if (watchingUnlock) {
            watchingUnlock = false
            unregisterReceiver(unlocked)
        }
    }

    private fun preferences(): SharedPreferences? =
        if (isUnlocked()) getSharedPreferences(Settings.FILE, MODE_PRIVATE) else null

    private fun memoryFile(): File? = if (isUnlocked()) File(filesDir, MEMORY_FILE) else null

    /** The local gesture trace, which only exists while recording is on. */
    private fun traceFile(): File? = preferences()
        ?.takeIf { it.getBoolean(Settings.TRACES, resources.getBoolean(R.bool.traces_by_default)) }
        ?.let { File(filesDir, TraceProvider.FILE) }

    private fun readAsset(name: String): ByteArray = assets.open(name).use { it.readBytes() }

    /** Loads what the core needs, once per process, and works before an unlock. */
    private fun loadLanguageData() {
        if (Core.loading.getAndSet(true)) return
        Core.onIo {
            keyboard.loadLexicon(readAsset("lexicon.csv"))
            keyboard.loadLanguage(readAsset("affinity.bin"), readAsset("forms.tsv"), readAsset("next.tsv"))
            handler.post { send(Event.LanguageReady) }
        }
    }

    /** Reads the user's memory and settings. After a reboot this waits for the unlock. */
    private fun restoreUserData() {
        val saved = preferences() ?: return
        tone = saved.getInt(Settings.TONE, 0)
        seenErasures = saved.getInt(Settings.ERASURES, 0)
        val recents = saved.getString(Settings.RECENTS, "").orEmpty().split(' ').filter(String::isNotEmpty)
        send(Event.Recents(recents))
        if (traceFile() != null) send(Event.EnableTraces)
        Core.onIo {
            val text = memoryFile()?.takeIf(File::exists)?.readText()
            handler.post { text?.let { send(Event.Memory(it)) } }
        }
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
        val saved = preferences() ?: return
        haptics = saved.getBoolean(Settings.HAPTICS, true)
        view?.showTrail = saved.getBoolean(Settings.TRAIL, true)
        send(Event.HeightScale(saved.getInt(Settings.HEIGHT, 100) / 100f))
        val erasures = saved.getInt(Settings.ERASURES, 0)
        if (erasures != seenErasures) {
            seenErasures = erasures
            send(Event.ForgetAll)
        }
    }

    private fun saveMemory(text: String) {
        val target = memoryFile() ?: return
        val temporary = File(filesDir, "$MEMORY_FILE.tmp")
        Core.onIo {
            temporary.writeText(text)
            temporary.renameTo(target)
        }
    }

    private fun appendTrace(line: String) {
        val target = traceFile() ?: return
        Core.onIo {
            if (target.length() < MAX_TRACE_BYTES) target.appendText(line + "\n")
        }
    }

    override fun onCreate() {
        super.onCreate()
        guarded { loadLanguageData() }
        guarded {
            if (isUnlocked()) {
                restoreUserData()
            } else {
                watchingUnlock = true
                registerReceiver(unlocked, IntentFilter(Intent.ACTION_USER_UNLOCKED), RECEIVER_NOT_EXPORTED)
            }
        }
    }

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        guarded { unregisterUnlockWatch() }
        view = null
        super.onDestroy()
    }

    override fun onCreateInputView(): View = KeyboardView(this, ::send).also {
        view = it
        it.onGestureMoved = ::requestPreview
        it.setEmoji(emoji)
        it.onToneChosen = { chosen ->
            tone = chosen
            preferences()?.edit()?.putInt(Settings.TONE, chosen)?.apply()
        }
    }

    override fun onEvaluateFullscreenMode(): Boolean = false

    override fun onStartInputView(info: EditorInfo, restarting: Boolean) {
        super.onStartInputView(info, restarting)
        guarded {
            applySettings()
            appendTrace("S\t${System.currentTimeMillis() / 1000}")
            val field = Field(info.inputType.toUInt(), info.imeOptions.toUInt())
            send(Event.Shown(field, textBeforeCursor(), textAfterCursor(), info.initialSelEnd.coerceAtLeast(0).toUInt()))
        }
    }

    override fun onFinishInputView(finishingInput: Boolean) {
        send(Event.Hidden)
        super.onFinishInputView(finishingInput)
    }

    override fun onUpdateSelection(oldStart: Int, oldEnd: Int, newStart: Int, newEnd: Int, composeStart: Int, composeEnd: Int) {
        super.onUpdateSelection(oldStart, oldEnd, newStart, newEnd, composeStart, composeEnd)
        send(Event.Selection(newStart.coerceAtLeast(0).toUInt(), newEnd.coerceAtLeast(0).toUInt()))
    }

    private fun textBeforeCursor(): String =
        currentInputConnection?.getTextBeforeCursor(CONTEXT_CHARS, 0)?.toString() ?: ""

    private fun textAfterCursor(): String =
        currentInputConnection?.getTextAfterCursor(CONTEXT_CHARS, 0)?.toString() ?: ""

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
            if (wantsContext) send(Event.Context(textBeforeCursor(), textAfterCursor()))
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
            is Command.Keys -> view?.setKeys(command.height, command.barHeight, command.keys)
            is Command.Look -> view?.setLook(command.pressed, command.shift.toInt())
            is Command.Popup -> view?.setPopup(command)
            is Command.ForgetBubble -> view?.setForgetBubble(command)
            is Command.Suggestions -> view?.setSuggestions(command)
            is Command.RequestContext -> Unit
            is Command.Trail -> view?.setTrail(command.on)
            is Command.EmojiPanel -> {
                if (command.on) loadEmoji()
                view?.setPanel(command.on, command.recents, tone)
            }
            is Command.EmojiSearch -> view?.setSearch(command.on, command.query, command.results)
            is Command.SaveMemory -> saveMemory(command.text)
            is Command.Trace -> appendTrace(command.line)
            is Command.SaveRecents ->
                preferences()?.edit()?.putString(Settings.RECENTS, command.items.joinToString(" "))?.apply()
            is Command.Announce -> view?.announceForAccessibility(spoken(command))
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
        const val CONTEXT_CHARS = 2000
        const val MEMORY_FILE = "memory.tsv"
        const val MAX_TRACE_BYTES = 64L * 1024 * 1024
    }
}
