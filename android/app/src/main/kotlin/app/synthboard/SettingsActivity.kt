package app.synthboard

import android.app.Activity
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
import java.io.File

/** Keys shared by the settings screen and the keyboard. */
object Settings {
    const val HAPTICS = "haptics"
    const val TRAIL = "trail"
    const val HEIGHT = "height"
    const val ERASURES = "erasures"
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

    private fun heading(text: String) {
        list.addView(TextView(this).apply {
            this.text = text
            textSize = 15f
            setPadding(0, (20 * resources.displayMetrics.density).toInt(), 0, 8)
        })
    }

    private fun setup() {
        val manager = getSystemService(InputMethodManager::class.java)
        val enabled = manager.enabledInputMethodList.any { it.packageName == packageName }
        val current = AndroidSettings.Secure.getString(contentResolver, AndroidSettings.Secure.DEFAULT_INPUT_METHOD)
        if (enabled && current.orEmpty().startsWith(packageName)) return
        heading("Mise en route")
        if (!enabled) {
            action("1. Activer Synthboard") {
                startActivity(Intent(AndroidSettings.ACTION_INPUT_METHOD_SETTINGS))
            }
        } else {
            action("2. Choisir Synthboard") { manager.showInputMethodPicker() }
        }
    }

    private fun typing() {
        heading("Saisie")
        toggle("Vibration des touches", Settings.HAPTICS)
        toggle("Ligne du swipe", Settings.TRAIL)
        val label = TextView(this)
        fun show(percent: Int) {
            label.text = "Hauteur du clavier : $percent %"
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
        heading("Ce que le clavier a appris")
        action("Exporter mes mots") {
            startActivityForResult(
                Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
                    .setType("text/tab-separated-values")
                    .putExtra(Intent.EXTRA_TITLE, "synthboard-mots.tsv"),
                EXPORT,
            )
        }
        action("Tout effacer") { eraseEverything() }
    }

    private fun eraseEverything() {
        File(filesDir, "memory.tsv").delete()
        File(filesDir, "traces.tsv").delete()
        store.edit()
            .remove("recents")
            .putInt(Settings.ERASURES, store.getInt(Settings.ERASURES, 0) + 1)
            .apply()
        list.addView(TextView(this).apply { text = "Effacé. Le clavier repart de zéro à sa prochaine ouverture." })
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

    private fun toggle(label: String, key: String) {
        list.addView(Switch(this).apply {
            text = label
            isChecked = store.getBoolean(key, true)
            setOnCheckedChangeListener { _: View, on: Boolean -> store.edit().putBoolean(key, on).apply() }
        })
    }

    private fun action(label: String, run: () -> Unit) {
        list.addView(Button(this).apply {
            text = label
            setOnClickListener { run() }
        })
    }

    private companion object {
        const val EXPORT = 1
    }
}
