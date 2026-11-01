package app.synthboard

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import java.io.File
import java.io.FileNotFoundException

/** Tests the screen and the file loan that hold the privacy promises. */
@RunWith(AndroidJUnit4::class)
@Config(sdk = [35])
class SettingsTest {
    private val context = ApplicationProvider.getApplicationContext<Context>()
    private val uri = Uri.parse("content://${TraceProvider.AUTHORITY}/${TraceProvider.FILE}")

    private fun buttons(view: View): List<Button> = when (view) {
        is Button -> listOf(view)
        is ViewGroup -> (0 until view.childCount).flatMap { buttons(view.getChildAt(it)) }
        else -> emptyList()
    }

    private fun provider(): TraceProvider = Robolectric.setupContentProvider(TraceProvider::class.java, TraceProvider.AUTHORITY)

    private fun press(activity: SettingsActivity, label: Int) {
        val text = context.getString(label)
        buttons(activity.window.decorView).first { it.text == text }.performClick()
    }

    @Test
    fun traces_go_to_tailscale_and_to_nobody_else() {
        val activity = Robolectric.buildActivity(SettingsActivity::class.java).setup().get()
        press(activity, R.string.send_traces)
        val intent = shadowOf(activity).nextStartedActivity
        assertEquals(Intent.ACTION_SEND, intent.action)
        assertEquals("com.tailscale.ipn", intent.`package`)
        assertTrue(intent.flags and Intent.FLAG_GRANT_READ_URI_PERMISSION != 0)
        assertFalse(intent.flags and Intent.FLAG_GRANT_WRITE_URI_PERMISSION != 0)
    }

    @Test
    fun erasing_removes_memory_and_traces_and_tells_the_keyboard() {
        File(context.filesDir, "memory.tsv").writeText("mot\t1\n")
        File(context.filesDir, TraceProvider.FILE).writeText("G\ta\n")
        val activity = Robolectric.buildActivity(SettingsActivity::class.java).setup().get()
        press(activity, R.string.erase)
        assertFalse(File(context.filesDir, "memory.tsv").exists())
        assertFalse(File(context.filesDir, TraceProvider.FILE).exists())
        val store = context.getSharedPreferences(Settings.FILE, Context.MODE_PRIVATE)
        assertEquals(1, store.getInt(Settings.ERASURES, 0))
    }

    @Test
    fun the_trace_file_is_lent_read_only() {
        File(context.filesDir, TraceProvider.FILE).writeText("trace")
        val loan = provider().openFile(uri, "r")
        val size = ParcelFileDescriptor.AutoCloseInputStream(loan).use { it.readBytes().size }
        assertEquals(5, size)
        var refused = false
        try {
            provider().openFile(uri, "w")
        } catch (_: FileNotFoundException) {
            refused = true
        }
        assertTrue(refused)
    }

    @Test
    fun the_shared_file_has_its_name_and_size() {
        File(context.filesDir, TraceProvider.FILE).writeText("trace")
        val cursor = provider().query(uri, null, null, null, null)
        cursor.moveToFirst()
        assertEquals(TraceProvider.FILE, cursor.getString(cursor.getColumnIndexOrThrow(OpenableColumns.DISPLAY_NAME)))
        assertEquals(5L, cursor.getLong(cursor.getColumnIndexOrThrow(OpenableColumns.SIZE)))
    }

    @Test
    fun nothing_can_be_written_through_the_provider() {
        assertNull(provider().insert(uri, null))
        assertEquals(0, provider().delete(uri, null, null))
        assertEquals(0, provider().update(uri, null, null, null))
    }
}
