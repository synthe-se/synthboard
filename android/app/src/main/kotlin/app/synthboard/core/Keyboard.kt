package app.synthboard.core

import java.nio.ByteBuffer

private object Native {
    init {
        System.loadLibrary("synthboard_ffi")
    }

    @JvmStatic
    external fun address(buffer: ByteBuffer): Long

    @JvmStatic
    external fun call(input: Long, size: Int, output: Long, capacity: Int): Int

    @JvmStatic
    external fun take(output: Long)
}

/** The keyboard, as Kotlin sees it: every call is one message to Rust and back. */
class Keyboard {
    /** One set per thread, shared with Rust, so that a call allocates nothing. */
    private class Buffers {
        val writer = WireWriter()
        var input = writer.buffer
        var inputAddress = Native.address(input)
        val output = directBuffer(OUTPUT_BYTES)
        val outputAddress = Native.address(output)

        /** Where the message is: writing it may have moved it to a larger buffer. */
        fun address(): Long {
            if (writer.buffer !== input) {
                input = writer.buffer
                inputAddress = Native.address(input)
            }
            return inputAddress
        }
    }

    private val buffers = ThreadLocal.withInitial(::Buffers)

    private fun send(call: Call): Reply {
        val set = buffers.get()
        val writer = set.writer
        call.write(writer)
        val size = Native.call(set.address(), writer.size, set.outputAddress, OUTPUT_BYTES)
        writer.clear()
        val reply = if (size <= OUTPUT_BYTES) set.output else directBuffer(size).also { Native.take(Native.address(it)) }
        reply.clear()
        return Reply.read(WireReader(reply)).also { reply.wipe(size) }
    }

    private fun done(call: Call) {
        send(call)
    }

    fun step(event: Event): Step = (send(Call.Step(event)) as Reply.Stepped).step

    fun mayRead(field: Field): Boolean = (send(Call.MayRead(field)) as Reply.Flag).on

    fun setTraceKey(hex: String) = done(Call.SetTraceKey(hex))

    fun preview(): String = (send(Call.Preview) as Reply.Text).text

    fun loadLanguage(affinities: ByteArray, forms: ByteArray, nextWords: ByteArray) =
        done(Call.LoadLanguage(affinities, forms, nextWords))

    fun loadEmoji(bytes: ByteArray): List<EmojiGroup> = (send(Call.LoadEmoji(bytes)) as Reply.Emoji).groups

    fun loadLexicon(bytes: ByteArray) = done(Call.LoadLexicon(bytes))

    fun newMemoryKeys(): ByteArray = (send(Call.NewMemoryKeys) as Reply.Bytes).bytes

    fun setMemoryKey(key: ByteArray) = done(Call.SetMemoryKey(key))

    fun loadMemory(sealed: ByteArray, secret: ByteArray) = done(Call.LoadMemory(sealed, secret))

    fun openMemory(sealed: ByteArray, secret: ByteArray): ByteArray =
        (send(Call.OpenMemory(sealed, secret)) as Reply.Bytes).bytes

    private companion object {
        const val OUTPUT_BYTES = 8 * 1024
    }
}
