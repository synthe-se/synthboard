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
    external fun preview(output: Long, capacity: Int): Int

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

    /** Only the preview thread uses these. */
    private val previewOutput by lazy { directBuffer(PREVIEW_BYTES) }
    private val previewAddress by lazy { Native.address(previewOutput) }

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

    fun preview(): String {
        val size = Native.preview(previewAddress, PREVIEW_BYTES)
        if (size == 0) return ""
        val bytes = ByteArray(size)
        previewOutput.clear()
        previewOutput.get(bytes)
        previewOutput.wipe(size)
        return String(bytes, Charsets.UTF_8).also { bytes.fill(0) }
    }

    fun loadLanguage(affinities: ByteArray, forms: ByteArray, nextWords: ByteArray) =
        done(Call.LoadLanguage(affinities, forms, nextWords))

    fun loadEmoji(bytes: ByteArray): List<EmojiGroup> = (send(Call.LoadEmoji(bytes)) as Reply.Emoji).groups

    fun loadLexicon(bytes: ByteArray) = done(Call.LoadLexicon(bytes))
    fun loadNeural(bytes: ByteArray) = done(Call.LoadNeural(bytes))

    fun newMemoryKeys(): ByteArray = (send(Call.NewMemoryKeys) as Reply.Bytes).bytes

    fun setMemoryKey(key: ByteArray) = done(Call.SetMemoryKey(key))

    /** False if the memory cannot be opened. No sealed bytes means there is none yet. */
    fun loadMemory(sealed: ByteArray?, secret: ByteArray): Boolean =
        (send(Call.LoadMemory(sealed, secret)) as Reply.Flag).on

    /** Memory to store, or nothing if a later save got there first. */
    fun sealMemory(): ByteArray = (send(Call.SealMemory) as Reply.Bytes).bytes

    fun openMemory(sealed: ByteArray, secret: ByteArray): ByteArray =
        (send(Call.OpenMemory(sealed, secret)) as Reply.Bytes).bytes

    private companion object {
        /** Holds any usual step, keys included: a larger reply goes through `take`. */
        const val OUTPUT_BYTES = 8192

        /** Far longer than a word, which is all a preview holds. */
        const val PREVIEW_BYTES = 256
    }
}
