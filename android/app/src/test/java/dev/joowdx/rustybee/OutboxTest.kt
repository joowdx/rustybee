package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.FrameHandler
import dev.joowdx.rustybee.gateway.Sender
import dev.joowdx.rustybee.gateway.Uplink
import dev.joowdx.rustybee.protocol.Ack
import dev.joowdx.rustybee.protocol.Envelope
import dev.joowdx.rustybee.protocol.Inbound
import dev.joowdx.rustybee.protocol.ProtocolJson
import dev.joowdx.rustybee.protocol.Status
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** AC-011-03 and the app half of AC-002-04. */
class OutboxTest {
    private val store = FakeOutbox()
    private val uplink = Uplink(store) { 1_760_000_000_000 }

    private fun status(n: Int) = Status("MSG$n", "sent", 1L, 1)
    private fun ids(frames: List<String>) = frames.map { ProtocolJson.decodeFromString(Envelope.serializer(), it).id }

    @Test
    fun ac_011_03_offline_frames_are_sent_in_order_after_reconnect() {
        val queued = (1..3).map { uplink.enqueueStatus(status(it)) }
        val wire = mutableListOf<String>()
        uplink.attach { wire += it; true }
        assertEquals(queued, ids(wire))
    }

    @Test
    fun ac_011_03_ack_removes_exactly_that_row() {
        val (a, b, c) = (1..3).map { uplink.enqueueStatus(status(it)) }
        assertTrue(uplink.onAck(b))
        assertEquals(listOf(a, c), store.pending().map { it.frameId })
        assertFalse(uplink.onAck(b))
        assertFalse(uplink.onAck("UNKNOWN"))
        assertEquals(2, uplink.size())
    }

    @Test
    fun ac_002_04_unacked_frames_resent_with_same_id_after_reconnect() {
        val first = mutableListOf<String>()
        uplink.attach { first += it; true }
        val a = uplink.enqueueStatus(status(1))
        val b = uplink.enqueueStatus(status(2))
        assertEquals(listOf(a, b), ids(first))
        uplink.onAck(a)
        uplink.detach() // connection dropped with b unacked

        val c = uplink.enqueueStatus(status(3)) // queued while offline, not sent
        val second = mutableListOf<String>()
        uplink.attach { second += it; true }
        assertEquals(listOf(b, c), ids(second))
        assertEquals(first[1], second[0]) // byte-identical frame, same id
    }

    @Test
    fun ac_011_03_frames_wait_for_welcome_before_replay() {
        uplink.enqueueStatus(status(1))
        val wire = mutableListOf<String>()
        val handler = FrameHandler(
            seen = FakeSeen(),
            uplink = uplink,
            sender = Sender(FakeSent(), uplink, FakeRadio(), { null }),
            onWelcome = { uplink.attach { wire += it; true } },
            onConfig = {},
        )
        val replies = mutableListOf<String>()
        handler.handle(Fixtures.read("welcome.json")) { replies += it }
        assertEquals(1, replies.size) // ack of welcome
        val ack0 = ProtocolJson.decodeFromString(Envelope.serializer(), replies[0])
        assertEquals("ack", ack0.type)
        assertEquals("01JA0000000000000000000002", ProtocolJson.decodeFromJsonElement(Ack.serializer(), ack0.data).ref)
        assertEquals(1, wire.size)

        // An ack from the server removes the row.
        val ack = """{"v":1,"id":"01JA00000000000000000000ZZ","type":"ack","data":{"ref":"${ids(wire)[0]}"}}"""
        handler.handle(ack) { replies += it }
        assertEquals(0, store.count())
        assertEquals(1, replies.size) // acks are never acked
    }

    @Test
    fun ac_012_inbound_with_queued_uid_is_a_noop() {
        val inbound = Inbound("3b0368d22af9d295abcbe52c2ce7fdaa", "+15551234567", "Your code is 123456", 1, 1_760_000_000_000)
        assertTrue(uplink.enqueueInbound(inbound))
        assertFalse(uplink.enqueueInbound(inbound))
        assertEquals(1, store.count())
    }
}
