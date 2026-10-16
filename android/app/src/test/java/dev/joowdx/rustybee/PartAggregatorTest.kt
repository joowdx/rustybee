package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.PartAggregator
import dev.joowdx.rustybee.gateway.SentRow
import dev.joowdx.rustybee.gateway.SentState
import dev.joowdx.rustybee.gateway.SmsErrors
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** AC-012-01. */
class PartAggregatorTest {
    private val start = SentRow("M1", "+15550001111", SentState.ACCEPTED, parts = 3, updatedAt = 0)

    private fun sent(vararg codes: Int): Pair<SentRow, List<String>> {
        var row = start
        val states = mutableListOf<String>()
        codes.forEachIndexed { i, c ->
            val r = PartAggregator.onSent(row, c, i.toLong())
            row = r.row
            states += r.statuses.map { it.state }
        }
        return row to states
    }

    @Test
    fun ac_012_01_ok_ok_failure_gives_one_failed() {
        val (row, states) = sent(SmsErrors.RESULT_OK, SmsErrors.RESULT_OK, SmsErrors.NO_SERVICE)
        assertEquals(listOf("failed"), states)
        assertEquals(SentState.FAILED, row.state)
        assertEquals("no_service", row.errorCode)
    }

    @Test
    fun ac_012_01_failure_first_still_waits_for_all_parts() {
        val (_, states) = sent(SmsErrors.RADIO_OFF, SmsErrors.RESULT_OK, SmsErrors.RESULT_OK)
        assertEquals(listOf("failed"), states)
    }

    @Test
    fun ac_012_01_ok_ok_ok_gives_sent_then_delivered() {
        var (row, states) = sent(SmsErrors.RESULT_OK, SmsErrors.RESULT_OK, SmsErrors.RESULT_OK)
        assertEquals(listOf("sent"), states)
        val delivered = mutableListOf<String>()
        repeat(3) {
            val r = PartAggregator.onDelivered(row, 0, 10)
            row = r.row
            delivered += r.statuses.map { it.state }
        }
        assertEquals(listOf("delivered"), delivered)
        assertEquals(SentState.DELIVERED, row.state)
    }

    @Test
    fun ac_012_01_pending_delivery_is_ignored_and_permanent_failure_fails() {
        var row = sent(SmsErrors.RESULT_OK, SmsErrors.RESULT_OK, SmsErrors.RESULT_OK).first
        val pending = PartAggregator.onDelivered(row, 0x20, 1)
        assertTrue(pending.statuses.isEmpty())
        assertEquals(row, pending.row)
        val failed = PartAggregator.onDelivered(row, 0x40, 2)
        assertEquals(listOf("failed"), failed.statuses.map { it.state })
        assertEquals("delivery_failed", failed.statuses[0].error?.code)
        row = failed.row
        assertTrue(PartAggregator.onDelivered(row, 0x45, 3).statuses.isEmpty()) // already failed
    }

    @Test
    fun ac_012_01_unknown_result_code_maps_to_unknown_n() {
        assertEquals("unknown_42", SmsErrors.fromResult(42).code)
        assertEquals("short_code_never_allowed", SmsErrors.fromResult(SmsErrors.SHORT_CODE_NEVER_ALLOWED).code)
    }
}
