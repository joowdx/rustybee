package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.Backoff
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** AC-011-04. */
class BackoffTest {
    @Test
    fun ac_011_04_sequence_doubles_to_120s() {
        val b = Backoff(random = { 0.5 }) // no jitter
        val seq = List(10) { b.nextDelayMs() }
        assertEquals(listOf(1_000L, 2_000, 4_000, 8_000, 16_000, 32_000, 64_000, 120_000, 120_000, 120_000), seq)
    }

    @Test
    fun ac_011_04_jitter_stays_within_20_percent() {
        for (r in listOf(0.0, 0.25, 0.75, 0.9999)) {
            val b = Backoff(random = { r })
            val nominal = listOf(1_000L, 2_000, 4_000, 8_000, 16_000, 32_000, 64_000, 120_000, 120_000)
            nominal.forEach { n ->
                val d = b.nextDelayMs()
                assertTrue("$d vs $n", d >= n * 0.8 - 1 && d <= n * 1.2 + 1)
            }
        }
    }

    @Test
    fun ac_011_04_resets_after_60s_up() {
        val b = Backoff(random = { 0.5 })
        repeat(5) { b.nextDelayMs() }
        b.onOpened(now = 0)
        b.onClosed(now = 60_000)
        assertEquals(1_000L, b.nextDelayMs())
    }

    @Test
    fun ac_011_04_short_connection_does_not_reset() {
        val b = Backoff(random = { 0.5 })
        repeat(3) { b.nextDelayMs() } // 1, 2, 4
        b.onOpened(now = 0)
        b.onClosed(now = 59_999)
        assertEquals(8_000L, b.nextDelayMs())
    }
}
