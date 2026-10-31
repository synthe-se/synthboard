package app.synthboard

import android.content.SharedPreferences
import android.content.pm.ApplicationInfo
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
import app.synthboard.core.Keyboard
import java.io.File
import java.util.concurrent.Executors

/**
 * Forwards Android events to the Rust core and applies its commands. The core
 * makes every decision, including when the single deadline is due.
 */
class SynthboardService : InputMethodService() {
    private val keyboard = Keyboard()
    private val handler = Handler(Looper.getMainLooper())
    private val deadline = Runnable { send(Event.Deadline(SystemClock.uptimeMillis().toULong())) }
    private val previewThread = Executors.newSingleThreadExecutor()
    private var previewRunning = false
    private var previewStale = false
    private var view: KeyboardView? = null
    private var emoji: List<Pair<String, List<EmojiItem>>> = emptyList()
    private var tone = 0
    private var haptics = true
    private var seenErasures = 0

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
        previewThread.execute {
            val word = keyboard.preview()
            handler.post {
                previewRunning = false
                view?.setPreview(word)
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

    /** The local gesture trace, written by debug builds only. */
    private fun traceFile(): File? =
        memoryFile()?.takeIf { applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0 }
            ?.let { File(filesDir, "traces.tsv") }

    /** What was learned. Only reachable once the user has unlocked the phone. */
    private fun memoryFile(): File? =
        if (getSystemService(UserManager::class.java).isUserUnlocked) File(filesDir, "memory.tsv") else null

    private fun saveMemory(text: String) {
        val target = memoryFile() ?: return
        val temporary = File(filesDir, "memory.tmp")
        temporary.writeText(text)
        temporary.renameTo(target)
    }

    /** Recent emoji and skin tone. Only readable once the user has unlocked. */
    private fun preferences(): SharedPreferences? =
        if (getSystemService(UserManager::class.java).isUserUnlocked) getSharedPreferences(Settings.FILE, MODE_PRIVATE) else null

    override fun onCreate() {
        super.onCreate()
        Thread {
            keyboard.loadLexicon(assets.open("lexicon.csv").use { it.readBytes() })
            keyboard.loadLanguage(
                assets.open("affinity.bin").use { it.readBytes() },
                assets.open("forms.tsv").use { it.readBytes() },
                assets.open("next.tsv").use { it.readBytes() },
            )
            val saved = memoryFile()?.takeIf { it.exists() }?.readText()
            handler.post {
                saved?.let { send(Event.Memory(it)) }
                send(Event.LanguageReady)
                if (traceFile() != null) send(Event.EnableTraces)
            }
        }.start()
        Thread {
            val paint = Paint()
            val groups = keyboard.loadEmoji(assets.open("emoji.tsv").use { it.readBytes() }).map { group ->
                group.name to group.items.filter { paint.hasGlyph(it.emoji) }
            }
            handler.post {
                emoji = groups
                view?.setEmoji(groups)
            }
        }.start()
        preferences()?.let { saved ->
            tone = saved.getInt("tone", 0)
            seenErasures = saved.getInt(Settings.ERASURES, 0)
            send(Event.Recents(saved.getString("recents", "").orEmpty().split(' ').filter(String::isNotEmpty)))
        }
    }

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        previewThread.shutdownNow()
        keyboard.close()
        super.onDestroy()
    }

    override fun onCreateInputView(): View = KeyboardView(this, ::send).also {
        view = it
        it.onGestureMoved = ::requestPreview
        it.setEmoji(emoji)
        it.onToneChosen = { chosen ->
            tone = chosen
            preferences()?.edit()?.putInt("tone", chosen)?.apply()
        }
    }

    override fun onEvaluateFullscreenMode(): Boolean = false

    override fun onStartInputView(info: EditorInfo, restarting: Boolean) {
        super.onStartInputView(info, restarting)
        applySettings()
        val field = Field(info.inputType.toUInt(), info.imeOptions.toUInt())
        send(Event.Shown(field, textBeforeCursor(), info.initialSelEnd.coerceAtLeast(0).toUInt()))
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

    private fun send(event: Event) {
        val step = keyboard.step(event)
        handler.removeCallbacks(deadline)
        step.deadline?.let { handler.postAtTime(deadline, it.toLong()) }

        val connection = currentInputConnection
        var wantsContext = false
        connection?.beginBatchEdit()
        for (command in step.commands) {
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
                is Command.Suggestions -> view?.setSuggestions(command)
                is Command.RequestContext -> wantsContext = true
                is Command.Trail -> view?.setTrail(command.on)
                is Command.EmojiPanel -> view?.setPanel(command.on, command.recents, tone)
                is Command.EmojiSearch -> view?.setSearch(command.on, command.query, command.results)
                is Command.SaveMemory -> saveMemory(command.text)
                is Command.Trace -> traceFile()?.appendText(command.line + "\n")
                is Command.SaveRecents -> preferences()?.edit()?.putString("recents", command.items.joinToString(" "))?.apply()
            }
        }
        connection?.endBatchEdit()
        if (wantsContext) send(Event.Context(textBeforeCursor()))
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
    }
}
