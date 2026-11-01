package app.synthboard

import android.content.ComponentName
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import java.io.File
import java.io.FileNotFoundException

/**
 * Lends the sealed trace to the one app the settings screen shares it with. It
 * is not exported, so nothing can read it without a grant made by a user's tap.
 */
class TraceProvider : ContentProvider() {
    private fun traces(): File = file(requireNotNull(context))

    override fun onCreate(): Boolean = true

    override fun getType(uri: Uri): String = TYPE

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        val file = traces()
        if (mode != "r" || !file.exists()) throw FileNotFoundException(uri.toString())
        return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
    }

    override fun query(uri: Uri, columns: Array<String>?, where: String?, arguments: Array<String>?, order: String?): Cursor {
        val names = arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
        return MatrixCursor(names).apply { addRow(arrayOf<Any>(FILE, traces().length())) }
    }

    override fun insert(uri: Uri, values: ContentValues?): Uri? = null

    override fun update(uri: Uri, values: ContentValues?, where: String?, arguments: Array<String>?): Int = 0

    override fun delete(uri: Uri, where: String?, arguments: Array<String>?): Int = 0

    companion object {
        const val FILE = "traces.sealed"
        const val TYPE = "application/octet-stream"

        /** Recording stops here, and the settings screen says so. */
        const val MAX_BYTES = 64L * 1024 * 1024

        /** The sealed trace. */
        fun file(context: Context) = File(context.filesDir, FILE)

        /** The manifest names the provider, and this reads it there. The old overload, which Robolectric can run. */
        @Suppress("DEPRECATION")
        fun authority(context: Context): String =
            context.packageManager.getProviderInfo(ComponentName(context, TraceProvider::class.java), 0).authority
    }
}
