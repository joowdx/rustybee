package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.Sender
import dev.joowdx.rustybee.gateway.SentRow
import dev.joowdx.rustybee.gateway.SentState
import dev.joowdx.rustybee.gateway.SmsErrors
import dev.joowdx.rustybee.gateway.Uplink
import dev.joowdx.rustybee.protocol.Envelope
import dev.joowdx.rustybee.protocol.ProtocolJson
import dev.joowdx.rustybee.protocol.Send
import dev.joowdx.rustybee.protocol.Status
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** AC-012-02 and the device `send` rules of spec 002. */
class SendDedupeTest {
    private val outbox = FakeOutbox()
    private val sent = FakeSent()
    private val radio = FakeRadio()
    private var now = 1_000L
    private var configSim: Int? = null
    private val sender = Sender(sent, Uplink(outbox) { now }, radio, { configSim }, { now })
    private val msg = Send("M1", "+15557654321", "hi", null)
    private var acks = 0

    private fun statuses(): List<Status> = outbox.pending().map {
        ProtocolJson.decodeFromJsonElement(Status.serializer(), ProtocolJson.decodeFromString(Envelope.serializer(), it.json).data)
    }

    @Test
    fun ac_012_02_terminal_outcome_is_rereported_without_sending_again() {
        sender.onSend(msg) { acks++ }
        assertEquals(1, radio.calls.size)
        sender.onSentResult("M1", SmsErrors.RESULT_OK)
        assertEquals(listOf("sent"), statuses().map { it.state })
        outbox.rows.clear() // server acked it

        sender.onSend(msg) { acks++ }
        assertEquals(1, radio.calls.size) // 0 more sends
        assertEquals(2, acks)
        assertEquals(listOf("sent"), statuses().map { it.state })
    }

    @Test
    fun ac_012_02_delivered_is_rereported_as_sent_then_delivered() {
        sender.onSend(msg) { acks++ }
        sender.onSentResult("M1", SmsErrors.RESULT_OK)
        sender.onDeliveredResult("M1", 0)
        outbox.rows.clear()
        sender.onSend(msg) { acks++ }
        assertEquals(1, radio.calls.size)
        assertEquals(listOf("sent", "delivered"), statuses().map { it.state })
    }

    @Test
    fun ac_012_02_in_progress_is_acked_only() {
        sender.onSend(msg) { acks++ }
        sender.onSend(msg) { acks++ }
        assertEquals(1, radio.calls.size)
        assertEquals(2, acks)
        assertTrue(outbox.rows.isEmpty())
        assertEquals(SentState.ACCEPTED, sent.get("M1")!!.state)
    }

    @Test
    fun ac_012_02_ack_happens_after_row_insert_and_before_radio() {
        var rowAtAck: SentRow? = null
        var callsAtAck = -1
        sender.onSend(msg) { rowAtAck = sent.get("M1"); callsAtAck = radio.calls.size }
        assertEquals(SentState.ACCEPTED, rowAtAck?.state)
        assertEquals(0, callsAtAck)
    }

    @Test
    fun ac_012_02_multipart_row_records_part_count() {
        radio.partsPerMessage = 3
        sender.onSend(msg) {}
        assertEquals(3, sent.get("M1")!!.parts)
        assertEquals(3, radio.calls.single().parts.size)
    }

    @Test
    fun inactive_sim_fails_with_sim_unavailable() {
        sender.onSend(msg.copy(sim = 9)) { acks++ }
        assertEquals(0, radio.calls.size)
        assertEquals(1, acks)
        assertEquals("sim_unavailable", statuses().single().error?.code)
    }

    @Test
    fun sim_falls_back_to_config_then_default() {
        configSim = 2
        sender.onSend(msg) {}
        assertEquals(2, radio.calls.last().sim)
        configSim = 7 // not active -> system default
        sender.onSend(msg.copy(msgId = "M2")) {}
        assertEquals(null, radio.calls.last().sim)
        sender.onSend(msg.copy(msgId = "M3", sim = 1)) {}
        assertEquals(1, radio.calls.last().sim)
    }

    @Test
    fun security_exception_fails_immediately() {
        radio.throwOnSend = SecurityException("no")
        sender.onSend(msg) {}
        assertEquals("permission_denied", statuses().single().error?.code)
        assertEquals(SentState.FAILED, sent.get("M1")!!.state)
    }

    @Test
    fun lost_outcome_after_10_minutes_is_failed_and_never_resent() {
        sender.onSend(msg) {}
        now += 10 * 60_000L + 1
        sender.failLost()
        val s = statuses().single()
        assertEquals("failed", s.state)
        assertEquals("outcome lost", s.error?.message)
        sender.onSend(msg) {}
        assertEquals(1, radio.calls.size)
    }
}
