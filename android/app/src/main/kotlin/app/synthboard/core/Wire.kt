package app.synthboard.core

import java.nio.ByteBuffer
import java.nio.ByteOrder

/** A buffer the garbage collector never moves, so no stray copy of what it holds is left behind. */
internal fun directBuffer(capacity: Int): ByteBuffer =
    ByteBuffer.allocateDirect(capacity).order(ByteOrder.LITTLE_ENDIAN)

internal fun ByteBuffer.wipe(size: Int) {
    for (index in 0 until size) put(index, 0)
}

/** Writes one message into `buffer`, which Rust reads and wipes. */
class WireWriter(private val allocate: (Int) -> ByteBuffer = ::directBuffer) {
    var buffer: ByteBuffer = allocate(INITIAL)
        private set

    val size: Int get() = buffer.position()

    /** Starts the next message, after Rust wiped this one. */
    fun clear() {
        if (buffer.capacity() > KEPT_BYTES) buffer = allocate(INITIAL)
        buffer.clear()
    }

    private fun room(count: Int) {
        if (buffer.remaining() >= count) return
        val old = buffer
        val used = old.position()
        val grown = allocate(maxOf(old.capacity() * 2, used + count))
        grown.put(old.duplicate().apply { flip() })
        old.wipe(used)
        buffer = grown
    }

    fun u8(value: UByte) {
        room(1)
        buffer.put(value.toByte())
    }

    fun u32(value: UInt) = i32(value.toInt())

    fun i32(value: Int) {
        room(Int.SIZE_BYTES)
        buffer.putInt(value)
    }

    fun u64(value: ULong) {
        room(Long.SIZE_BYTES)
        buffer.putLong(value.toLong())
    }

    fun f32(value: Float) = i32(value.toRawBits())

    fun bool(value: Boolean) = u8(if (value) 1u else 0u)

    fun bytes(value: ByteArray) {
        i32(value.size)
        room(value.size)
        buffer.put(value)
    }

    /** Lone surrogates become `?` here, and Rust checks the rest. */
    fun string(value: String) {
        val encoded = value.toByteArray(Charsets.UTF_8)
        bytes(encoded)
        encoded.fill(0)
    }

    fun <T> list(value: List<T>, item: (T) -> Unit) {
        i32(value.size)
        value.forEach(item)
    }

    fun <T : Any> optional(value: T?, item: (T) -> Unit) {
        bool(value != null)
        value?.let(item)
    }

    private companion object {
        const val INITIAL = 4 * 1024
    }
}

/** Reads one message from Rust. */
class WireReader(private val buffer: ByteBuffer) {
    fun u8(): UByte = buffer.get().toUByte()

    fun u32(): UInt = i32().toUInt()

    fun i32(): Int = buffer.getInt()

    fun u64(): ULong = buffer.getLong().toULong()

    fun f32(): Float = buffer.getFloat()

    fun bool(): Boolean = u8() != 0.toUByte()

    fun bytes(): ByteArray = ByteArray(i32()).also(buffer::get)

    fun string(): String {
        val bytes = bytes()
        return String(bytes, Charsets.UTF_8).also { bytes.fill(0) }
    }

    fun <T> list(item: () -> T): List<T> = List(i32()) { item() }

    fun <T> optional(item: () -> T): T? = if (bool()) item() else null
}
