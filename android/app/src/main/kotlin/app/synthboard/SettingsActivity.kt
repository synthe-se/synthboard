package app.synthboard

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.Settings as AndroidSettings
import android.view.View
import android.view.inputmethod.InputMethodManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.SeekBar
import android.widget.Switch
import android.widget.TextView
import android.widget.Toast
import java.io.File

/** Keys shared by the settings screen and the keyboard. */
object Settings {
    const val HAPTICS = "haptics"
    const val TRAIL = "trail"
    const val HEIGHT = "height"
    const val ERASURES = "erasures"
    const val TRACES = "traces"
    const val TONE = "tone"
    const val RECENTS = "recents"
    const val FILE = "emoji"
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
            val gutter = (20 * resources.displayMetrics.density).toInt()
            setPadding(gutter, gutter, gutter, gutter)
        }
        setContentView(ScrollView(this).apply { addView(list) })
    }

    override fun onResume() {
        super.onResume()
        list.removeAllViews()
        setup()
        typing()
        learned()
    }

    private fun heading(text: Int) {
        list.addView(TextView(this).apply {
            setText(text)
            textSize = 15f
            setPadding(0, (20 * resources.displayMetrics.density).toInt(), 0, 8)
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
        val label = TextView(this)
        fun show(percent: Int) {
            label.text = getString(R.string.height, percent)
        }
        show(store.getInt(Settings.HEIGHT, 100))
        list.addView(label)
        list.addView(SeekBar(this).apply {
            max = 50
            progress = store.getInt(Settings.HEIGHT, 100) - 75
            setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(bar: SeekBar, value: Int, fromUser: Boolean) {
                    show(value + 75)
                    if (fromUser) store.edit().putInt(Settings.HEIGHT, value + 75).apply()
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
                    .putExtra(Intent.EXTRA_TITLE, "synthboard-mots.tsv"),
                EXPORT,
            )
        }
        action(R.string.erase) { eraseEverything() }
        heading(R.string.development)
        toggle(R.string.record_traces, Settings.TRACES, resources.getBoolean(R.bool.traces_by_default))
        action(R.string.send_traces) { sendTraces() }
    }

    /**
     * Hands the trace to Tailscale and no other app. The keyboard has no network
     * access of its own: the file only moves on this tap, over the user's tailnet.
     */
    private fun sendTraces() {
        val uri = Uri.Builder().scheme("content").authority(TraceProvider.AUTHORITY).path(TraceProvider.FILE).build()
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

    private fun eraseEverything() {
        File(filesDir, "memory.tsv").delete()
        File(filesDir, TraceProvider.FILE).delete()
        store.edit()
            .remove(Settings.RECENTS)
            .putInt(Settings.ERASURES, store.getInt(Settings.ERASURES, 0) + 1)
            .apply()
        list.addView(TextView(this).apply { setText(R.string.erased) })
    }

    override fun onActivityResult(request: Int, result: Int, data: Intent?) {
        super.onActivityResult(request, result, data)
        val target: Uri = data?.data ?: return
        if (request != EXPORT || result != RESULT_OK) return
        val memory = File(filesDir, "memory.tsv")
        contentResolver.openOutputStream(target)?.use { out ->
            if (memory.exists()) out.write(memory.readBytes())
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
        const val TAILSCALE = "com.tailscale.ipn"
    }
}
