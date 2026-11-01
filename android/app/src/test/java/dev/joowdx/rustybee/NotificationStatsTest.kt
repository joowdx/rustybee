package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.Counts
import dev.joowdx.rustybee.gateway.NotificationText
import dev.joowdx.rustybee.gateway.Sender
import dev.joowdx.rustybee.gateway.SmsErrors
import dev.joowdx.rustybee.gateway.StatsPeriod
import dev.joowdx.rustybee.gateway.StatsTracker
import dev.joowdx.rustybee.gateway.Uplink
import dev.joowdx.rustybee.protocol.Send
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Spec 016 counts, requests and text: AC-016-06 to AC-016-09. */
class NotificationStatsTest {
    private val daily = FakeDaily()
    private val stats = StatsTracker(daily)
    private val day = 20_400L
    private val since = 1_000L

    @Test
    fun ac_016_06_reply_plus_local_events_since_the_request() {
        daily.bumpDaily(day, 2, 1)
        assertEquals(Counts(2, 1), stats.shown(since, day)) // no reply yet: local alone
        stats.onWelcome()
        stats.requested("R1", since, day)
        daily.bumpDaily(day, 1, 0) // after the request, before the reply
        stats.onStats(since, 120, 34)
        assertEquals(Counts(121, 34), stats.shown(since, day))
        daily.bumpDaily(day, 0, 2)
        assertEquals(Counts(121, 36), stats.shown(since, day))
    }

    @Test
    fun ac_016_06_rollover_drops_the_old_reply() {
        stats.onWelcome()
        stats.requested("R1", since, day)
        stats.onStats(since, 50, 5)
        daily.bumpDaily(day + 1, 1, 0)
        assertEquals(Counts(1, 0), stats.shown(2_000L, day + 1))
    }

    @Test
    fun ac_016_06_reply_for_another_period_is_ignored() {
        stats.onWelcome()
        stats.requested("R1", since, day)
        stats.onStats(null, 999, 999)
        assertEquals(Counts(), stats.shown(since, day))
    }

    @Test
    fun ac_016_07_requests_wait_for_welcome_and_an_empty_outbox() {
        assertFalse(stats.ready(welcomed = true, outbox = 0)) // nothing due before welcome
        stats.onWelcome()
        assertFalse(stats.ready(welcomed = false, outbox = 0))
        assertFalse(stats.ready(welcomed = true, outbox = 2))
        assertTrue(stats.ready(welcomed = true, outbox = 0))
        stats.requested("R1", since, day)
        stats.wanted = true // a health while one is in flight
        assertFalse(stats.ready(welcomed = true, outbox = 0))
        stats.onStats(since, 1, 1)
        assertTrue(stats.ready(welcomed = true, outbox = 0))
    }

    @Test
    fun ac_016_07_unsupported_stops_until_next_welcome() {
        stats.onWelcome()
        stats.requested("R1", since, day)
        assertFalse(stats.onNack("other", "invalid"))
        assertTrue(stats.onNack("R1", "unsupported"))
        stats.wanted = true
        assertFalse(stats.ready(welcomed = true, outbox = 0))
        stats.onWelcome()
        assertTrue(stats.ready(welcomed = true, outbox = 0))
    }

    @Test
    fun ac_016_07_lost_connection_drops_the_request() {
        stats.onWelcome()
        stats.requested("R1", since, day)
        stats.onClosed()
        stats.onStats(since, 9, 9) // a late reply is not applied
        assertEquals(Counts(), stats.shown(since, day))
    }

    @Test
    fun ac_016_08_text() {
        val c = Counts(120, 34)
        assertEquals(
            "This week: 120 sent, 34 received" to null,
            NotificationText.body(true, StatsPeriod.WEEK, c, 0, null),
        )
        assertEquals(
            "Today: 120 sent, 34 received, 3 queued" to null,
            NotificationText.body(true, StatsPeriod.TODAY, c, 3, "stale"),
        )
        assertEquals(
            "1006 timeout" to "1006 timeout\nAll time: 120 sent, 34 received, 3 queued",
            NotificationText.body(false, StatsPeriod.ALL, c, 3, "1006 timeout"),
        )
        val (text, expanded) = NotificationText.body(false, StatsPeriod.MONTH, c, 0, null)
        assertEquals("This month: 120 sent, 34 received", text)
        assertNull(expanded)
    }

    @Test
    fun ac_016_09_sent_counts_once_per_message() {
        var counted = 0
        val sender = Sender(FakeSent(), Uplink(FakeOutbox()) { 0 }, FakeRadio(), { null }, { 0 }, countSent = { counted++ })
        sender.onSend(Send("M1", "+15557654321", "hi", null)) {}
        assertEquals(0, counted) // handed to the radio is not sent yet
        sender.onSentResult("M1", SmsErrors.RESULT_OK)
        sender.onDeliveredResult("M1", 0)
        sender.onSend(Send("M1", "+15557654321", "hi", null)) {} // replay
        assertEquals(1, counted)

        sender.onSend(Send("M2", "+15557654321", "hi", null)) {}
        sender.onSentResult("M2", 1) // generic failure
        assertEquals(1, counted)
    }
}
