package se.synthe.synthboard

import se.synthe.synthboard.core.Command
import se.synthe.synthboard.core.Event
import se.synthe.synthboard.core.Step
import se.synthe.synthboard.core.WireReader
import se.synthe.synthboard.core.WireWriter
import org.junit.Assert.assertEquals
import org.junit.Test
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** The same bytes as the golden test in `codec.rs`, so the two sides cannot drift. */
class WireTest {
    private fun heap(size: Int): ByteBuffer = ByteBuffer.allocate(size).order(ByteOrder.LITTLE_ENDIAN)

    private fun written(work: (WireWriter) -> Unit): String {
        val writer = WireWriter(::heap).also(work)
        return writer.buffer.array().take(writer.size).joinToString("") { "%02x".format(it) }
    }

    private fun bytes(text: String) = text.chunked(2).map { it.toInt(16).toByte() }.toByteArray()

    @Test
    fun an_event_is_written_as_rust_reads_it() {
        assertEquals("050100000000000040000040400400000000000000", written(Event.Down(1, 2f, 3f, 4u)::write))
    }

    @Test
    fun a_step_is_read_as_rust_writes_it() {
        val message = ByteBuffer.wrap(bytes("010000000002000000c3a9010900000000000000")).order(ByteOrder.LITTLE_ENDIAN)
        val step = Step.read(WireReader(message))
        assertEquals(Step(listOf(Command.Commit("é")), 9u), step)
    }

    @Test
    fun a_lone_surrogate_never_reaches_rust_as_invalid_text() {
        assertEquals("02000000613f", written { it.string("a\uD83D") })
    }

    @Test
    fun a_message_larger_than_the_buffer_moves_and_stays_whole() {
        val writer = WireWriter(::heap)
        val small = writer.buffer
        writer.bytes(ByteArray(10_000) { 7 })
        assertEquals(10_004, writer.size)
        assertEquals(0, small.array().sum())
    }
}
