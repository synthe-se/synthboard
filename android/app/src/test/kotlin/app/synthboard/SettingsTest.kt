package app.synthboard

import android.content.Context
import android.content.Intent
import android.content.res.AssetManager
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import android.text.InputType
import android.view.View
import android.view.ViewGroup
import android.widget.EditText
import android.widget.Button
import android.widget.TextView
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
import org.robolectric.shadows.ShadowAlertDialog
import java.io.File
import java.io.FileNotFoundException
import java.security.GeneralSecurityException
import javax.crypto.KeyGenerator

/** Tests the screen and the file loan that hold the privacy promises. */
@RunWith(AndroidJUnit4::class)
@Config(sdk = [35])
class SettingsTest {
    private val context = ApplicationProvider.getApplicationContext<Context>()
    private val uri = Uri.parse("content://${TraceProvider.authority(context)}/${TraceProvider.FILE}")

    /** Sees no asset at all, as a public build sees no trace key, whatever this tree holds. */
    class PublicBuild : SettingsActivity() {
        private val empty = AssetManager::class.java.getDeclaredConstructor().apply { isAccessible = true }.newInstance()

        override fun getAssets(): AssetManager = empty
    }

    private fun texts(view: View): List<String> = when (view) {
        is TextView -> listOf(view.text.toString())
        is ViewGroup -> (0 until view.childCount).flatMap { texts(view.getChildAt(it)) }
        else -> emptyList()
    }

    private fun buttons(view: View): List<Button> = when (view) {
        is Button -> listOf(view)
        is ViewGroup -> (0 until view.childCount).flatMap { buttons(view.getChildAt(it)) }
        else -> emptyList()
    }

    private fun provider(): TraceProvider = Robolectric.setupContentProvider(TraceProvider::class.java, TraceProvider.authority(context))

    private fun press(activity: SettingsActivity, label: Int) {
        val text = context.getString(label)
        buttons(activity.window.decorView).first { it.text == text }.performClick()
    }

    @Test
    fun traces_go_to_tailscale_and_to_nobody_else() {
        val activity = Robolectric.buildActivity(SettingsActivity::class.java).setup().get()
        activity.sendTraces()
        val intent = shadowOf(activity).nextStartedActivity
        assertEquals(Intent.ACTION_SEND, intent.action)
        assertEquals("com.tailscale.ipn", intent.`package`)
        assertTrue(intent.flags and Intent.FLAG_GRANT_READ_URI_PERMISSION != 0)
        assertFalse(intent.flags and Intent.FLAG_GRANT_WRITE_URI_PERMISSION != 0)
    }

    @Test
    fun erasing_removes_memory_and_traces() {
        val personal = Settings.personalFiles(context)
        personal.forEach { it.writeText("x") }
        val activity = Robolectric.buildActivity(SettingsActivity::class.java).setup().get()
        press(activity, R.string.erase)
        val done = java.util.concurrent.CountDownLatch(1)
        Core.onIo { done.countDown() }
        assertTrue(done.await(5, java.util.concurrent.TimeUnit.SECONDS))
        personal.forEach { assertFalse(it.name, it.exists()) }
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
    fun a_build_without_an_owner_key_offers_no_recording_at_all() {
        val activity = Robolectric.buildActivity(SettingsActivity::class.java).setup().get()
        val labels = buttons(activity.window.decorView).map { it.text.toString() }
        assertEquals(Settings.hasTraceKey(context), context.getString(R.string.send_traces) in labels)
    }

    @Test
    fun a_public_build_has_no_development_section() {
        val activity = Robolectric.buildActivity(PublicBuild::class.java).setup().get()
        assertFalse(Settings.hasTraceKey(activity))
        val shown = texts(activity.window.decorView)
        assertTrue(context.getString(R.string.learned) in shown)
        assertFalse(context.getString(R.string.development) in shown)
        assertFalse(context.getString(R.string.record_traces) in shown)
        assertFalse(context.getString(R.string.send_traces) in shown)
    }

    @Test
    fun the_licences_can_be_read_in_the_app() {
        val activity = Robolectric.buildActivity(SettingsActivity::class.java).setup().get()
        press(activity, R.string.third_party)
        val shown = shadowOf(ShadowAlertDialog.getLatestAlertDialog()).message.toString()
        assertEquals(context.assets.open("third_party.txt").use { it.readBytes().decodeToString() }, shown)
        assertTrue("ONNX Runtime" in shown)
        assertFalse("KleidiAI" in shown)
    }

    @Test
    fun memory_can_be_read_back_with_its_key_and_only_with_it() {
        val generator = KeyGenerator.getInstance("AES").apply { init(256) }
        val key = generator.generateKey()
        val vault = Vault { key }
        val sealed = vault.seal("johnduff".toByteArray())
        assertEquals("johnduff", String(vault.open(sealed)))
        assertFalse(String(sealed, Charsets.ISO_8859_1).contains("johnduff"))
        var refused = false
        try {
            val other = generator.generateKey()
            Vault { other }.open(sealed)
        } catch (_: GeneralSecurityException) {
            refused = true
        }
        assertTrue(refused)
    }

    @Test
    fun a_personal_build_has_a_field_to_try_the_keyboard_in() {
        val activity = Robolectric.buildActivity(SettingsActivity::class.java).setup().get()
        fun fields(view: View): List<EditText> = when (view) {
            is EditText -> listOf(view)
            is ViewGroup -> (0 until view.childCount).flatMap { fields(view.getChildAt(it)) }
            else -> emptyList()
        }
        val found = fields(activity.window.decorView)
        assertEquals(Settings.hasTraceKey(context), found.isNotEmpty())
        found.forEach { assertTrue(it.inputType and InputType.TYPE_TEXT_FLAG_MULTI_LINE != 0) }
    }

    @Test
    fun a_trace_file_at_its_limit_says_so_and_can_be_deleted() {
        val traces = File(context.filesDir, TraceProvider.FILE)
        java.io.RandomAccessFile(traces, "rw").use { it.setLength(TraceProvider.MAX_BYTES) }
        val activity = Robolectric.buildActivity(SettingsActivity::class.java).setup().get()
        if (!Settings.hasTraceKey(context)) return
        press(activity, R.string.delete_traces)
        assertFalse(traces.exists())
    }

    @Test
    fun the_memory_format_is_part_of_what_is_sealed() {
        val key = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
        val sealed = Vault { key }.seal("mot".toByteArray())
        val cipher = javax.crypto.Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(
            javax.crypto.Cipher.DECRYPT_MODE,
            key,
            javax.crypto.spec.GCMParameterSpec(128, sealed.copyOfRange(0, 12)),
        )
        var refused = false
        try {
            cipher.doFinal(sealed, 12, sealed.size - 12)
        } catch (_: GeneralSecurityException) {
            refused = true
        }
        assertTrue(refused)
    }

    @Test
    fun nothing_can_be_written_through_the_provider() {
        assertNull(provider().insert(uri, null))
        assertEquals(0, provider().delete(uri, null, null))
        assertEquals(0, provider().update(uri, null, null, null))
    }
}
