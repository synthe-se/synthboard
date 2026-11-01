package app.synthboard

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.Settings as AndroidSettings
import android.text.InputType
import android.view.Gravity
import android.view.View
import android.view.inputmethod.InputMethodManager
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.SeekBar
import android.widget.Switch
import android.widget.TextView
import android.widget.Toast
import app.synthboard.core.Event
import java.io.File
import java.io.IOException

/** Keys shared by the settings screen and the keyboard. */
object Settings {
    const val HAPTICS = "haptics"
    const val TRAIL = "trail"
    const val HEIGHT = "height"
    const val FULL_HEIGHT = 100
    const val QWERTY = "qwerty"
    const val TRACES = "traces"
    const val TONE = "tone"
    const val RECENTS = "recents"
    const val FILE = "emoji"
    const val SCREENSHOTS = "screenshots"
    const val NEURAL = "neural"
    const val MEMORY_FILE = "memory.sealed"
    const val MEMORY_SECRET_FILE = "memory.key"
    const val MEMORY_PUBLIC_FILE = "memory.pub"
    const val DAMAGED_MEMORY_FILE = "memory.damaged"
    private const val TRACE_KEY_ASSET = "trace.pub"

    /** The owner's public key, which only a personal build carries. */
    fun traceKey(context: Context): String? = try {
        context.assets.open(TRACE_KEY_ASSET).use { it.readBytes().decodeToString() }
    } catch (_: IOException) {
        null
    }

    fun hasTraceKey(context: Context): Boolean = traceKey(context) != null

    /** Every file holding something the keyboard learned or recorded. */
    fun personalFiles(context: Context): List<File> =
        listOf(MEMORY_FILE, "$MEMORY_FILE.tmp", MEMORY_SECRET_FILE, MEMORY_PUBLIC_FILE, DAMAGED_MEMORY_FILE, TraceProvider.FILE)
            .map { File(context.filesDir, it) }
}

/**
 * The only settings surface: the app icon, or the gear in Android's keyboard
 * settings. Nothing about settings appears on the keyboard itself.
 */
class SettingsActivity : Activity() {
    private val store by lazy { getSharedPreferences(Settings.FILE, Context.MODE_PRIVATE) }
    private lateinit var list: LinearLayout

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        title = "Synthboard"
        list = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            val gutter = (GUTTER_DP * resources.displayMetrics.density).toInt()
            setPadding(gutter, gutter, gutter, gutter)
        }
        setContentView(ScrollView(this).apply {
            filterTouchesWhenObscured = true
            setAccessibilityDataSensitive(View.ACCESSIBILITY_DATA_SENSITIVE_YES)
            addView(list)
        })
    }

    override fun onResume() {
        super.onResume()
        list.removeAllViews()
        setup()
        typing()
        learned()
    }

    /** A place to type without leaving the app, while working on the keyboard. */
    private fun tryOut() = EditText(this).apply {
        setHint(R.string.try_out)
        inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE or
            InputType.TYPE_TEXT_FLAG_CAP_SENTENCES
        minLines = TRY_OUT_LINES
        gravity = Gravity.TOP
        importantForAutofill = View.IMPORTANT_FOR_AUTOFILL_NO
    }

    private fun heading(text: Int) {
        list.addView(TextView(this).apply {
            setText(text)
            textSize = HEADING_SP
            setPadding(0, (GUTTER_DP * resources.displayMetrics.density).toInt(), 0, HEADING_GAP)
        })
    }

    private fun setup() {
        val manager = getSystemService(InputMethodManager::class.java)
        val enabled = manager.enabledInputMethodList.any { it.packageName == packageName }
        val current = AndroidSettings.Secure.getString(contentResolver, AndroidSettings.Secure.DEFAULT_INPUT_METHOD)
        if (enabled && current.orEmpty().startsWith(packageName)) return
        heading(R.string.setup)
        if (!enabled) {
            action(R.string.enable) {
                startActivity(Intent(AndroidSettings.ACTION_INPUT_METHOD_SETTINGS))
            }
        } else {
            action(R.string.choose) { manager.showInputMethodPicker() }
        }
    }

    private fun typing() {
        heading(R.string.typing)
        toggle(R.string.haptics, Settings.HAPTICS)
        toggle(R.string.trail, Settings.TRAIL)
        toggle(R.string.qwerty, Settings.QWERTY, false)
        val label = TextView(this)
        fun show(percent: Int) {
            label.text = getString(R.string.height, percent)
        }
        show(store.getInt(Settings.HEIGHT, Settings.FULL_HEIGHT))
        list.addView(label)
        list.addView(SeekBar(this).apply {
            max = MAX_HEIGHT - MIN_HEIGHT
            progress = store.getInt(Settings.HEIGHT, Settings.FULL_HEIGHT) - MIN_HEIGHT
            setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(bar: SeekBar, value: Int, fromUser: Boolean) {
                    show(value + MIN_HEIGHT)
                    if (fromUser) store.edit().putInt(Settings.HEIGHT, value + MIN_HEIGHT).apply()
                }

                override fun onStartTrackingTouch(bar: SeekBar) = Unit
                override fun onStopTrackingTouch(bar: SeekBar) = Unit
            })
        })
    }

    private fun learned() {
        heading(R.string.learned)
        action(R.string.export) {
            startActivityForResult(
                Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
                    .setType("text/tab-separated-values")
                    .putExtra(Intent.EXTRA_TITLE, getString(R.string.export_file)),
                EXPORT,
            )
        }
        action(R.string.erase) { eraseEverything() }
        if (Settings.hasTraceKey(this)) {
            heading(R.string.development)
            list.addView(tryOut())
            toggle(R.string.record_traces, Settings.TRACES)
            toggle(R.string.screenshots, Settings.SCREENSHOTS, false)
            toggle(R.string.neural, Settings.NEURAL, false)
            val traces = File(filesDir, TraceProvider.FILE)
            if (traces.length() >= TraceProvider.MAX_BYTES) {
                list.addView(TextView(this).apply { setText(R.string.traces_full) })
            }
            action(R.string.send_traces) { sendTraces() }
            action(R.string.delete_traces) {
                traces.delete()
                list.addView(TextView(this).apply { setText(R.string.traces_deleted) })
            }
        }
    }

    /**
     * Hands the sealed trace to Tailscale and no other app. The keyboard has no
     * network access of its own: the file only moves on this tap, and only the
     * owner's secret key can open it.
     */
    internal fun sendTraces() {
        val authority = packageName + TraceProvider.AUTHORITY_SUFFIX
        val uri = Uri.Builder().scheme("content").authority(authority).path(TraceProvider.FILE).build()
        val share = Intent(Intent.ACTION_SEND)
            .setPackage(TAILSCALE)
            .setType(TraceProvider.TYPE)
            .putExtra(Intent.EXTRA_STREAM, uri)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        try {
            startActivity(share)
        } catch (_: ActivityNotFoundException) {
            Toast.makeText(this, R.string.no_tailscale, Toast.LENGTH_LONG).show()
        }
    }

    /**
     * The core forgets first, so a later hide has nothing to save, and the
     * files go on the I/O thread, after any save already queued.
     */
    private fun eraseEverything() {
        guarded { Core.keyboard.step(Event.ForgetAll) }
        val files = Settings.personalFiles(this)
        Core.onIo {
            files.forEach(File::delete)
            Vault.forget()
        }
        store.edit().remove(Settings.RECENTS).apply()
        list.addView(TextView(this).apply { setText(R.string.erased) })
    }

    override fun onActivityResult(request: Int, result: Int, data: Intent?) {
        super.onActivityResult(request, result, data)
        val target: Uri = data?.data ?: return
        if (request != EXPORT || result != RESULT_OK) return
        guarded {
            val memory = File(filesDir, Settings.MEMORY_FILE)
            val key = File(filesDir, Settings.MEMORY_SECRET_FILE)
            if (!memory.exists() || !key.exists()) return@guarded
            val plain = Core.keyboard.openMemory(memory.readBytes(), Vault.ofDevice().open(key.readBytes()))
            contentResolver.openOutputStream(target)?.use { it.write(plain) }
            plain.fill(0)
        }
    }

    private fun toggle(label: Int, key: String, default: Boolean = true) {
        list.addView(Switch(this).apply {
            setText(label)
            isChecked = store.getBoolean(key, default)
            setOnCheckedChangeListener { _: View, on: Boolean -> store.edit().putBoolean(key, on).apply() }
        })
    }

    private fun action(label: Int, run: () -> Unit) {
        list.addView(Button(this).apply {
            setText(label)
            setOnClickListener { run() }
        })
    }

    private companion object {
        const val EXPORT = 1
        const val TRY_OUT_LINES = 4
        const val GUTTER_DP = 20
        const val HEADING_SP = 15f
        const val HEADING_GAP = 8
        const val MIN_HEIGHT = 75
        const val MAX_HEIGHT = 125
        const val TAILSCALE = "com.tailscale.ipn"
    }
}
