package app.synthboard

import app.synthboard.core.Keyboard
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * What outlives a keyboard window. The Rust core lasts as long as the process,
 * so no background work can ever reach a closed core.
 */
object Core {
    val keyboard = Keyboard()

    /** One thread for files and loading, in order, away from the typing thread. */
    private val io = Executors.newSingleThreadExecutor()

    /** One thread for the live preview, which must never queue behind a file. */
    private val preview = Executors.newSingleThreadExecutor()

    /** Set once the language data has started loading, in this process. */
    val loading = AtomicBoolean(false)

    fun onIo(work: () -> Unit) = io.execute { guarded(work) }

    fun onPreview(work: () -> Unit) = preview.execute { guarded(work) }
}

/**
 * Stops any failure here. A keyboard must keep typing whatever goes wrong
 * around it, so a failed load or write only loses that one feature.
 */
inline fun guarded(work: () -> Unit) {
    try {
        work()
    } catch (_: Throwable) {
        // Nothing is logged: a message could hold typed text.
    }
}
