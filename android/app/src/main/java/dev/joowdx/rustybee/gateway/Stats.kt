package dev.joowdx.rustybee.gateway

import java.time.DayOfWeek
import java.time.LocalDate
import java.time.ZoneId
import java.time.temporal.TemporalAdjusters
import java.time.temporal.WeekFields
import java.util.Locale

/** The period the gateway notification counts (spec 016). */
enum class StatsPeriod(val key: String, val label: String) {
    TODAY("today", "Today"),
    WEEK("week", "This week"),
    MONTH("month", "This month"),
    ALL("all", "All time"),
    ;

    companion object {
        fun of(key: String?): StatsPeriod = entries.firstOrNull { it.key == key } ?: TODAY
    }
}

data class Counts(val sent: Int = 0, val received: Int = 0) {
    operator fun plus(o: Counts) = Counts(sent + o.sent, received + o.received)
    operator fun minus(o: Counts) = Counts(sent - o.sent, received - o.received)
}

/** The phone's own sent and received counts per local day (spec 016). */
interface DailyStore {
    fun bumpDaily(day: Long, sent: Int, received: Int)

    /** The sum of every day from [fromDay] on, or of all days when null. */
    fun sumDaily(fromDay: Long?): Counts
}

/** Calendar periods in the phone's time zone (spec 016). */
object StatsPeriods {
    /** The first day of the period holding [today], or null for all time. */
    fun startDay(period: StatsPeriod, weekStart: DayOfWeek, today: LocalDate): LocalDate? = when (period) {
        StatsPeriod.TODAY -> today
        StatsPeriod.WEEK -> today.with(TemporalAdjusters.previousOrSame(weekStart))
        StatsPeriod.MONTH -> today.withDayOfMonth(1)
        StatsPeriod.ALL -> null
    }

    /** The first day of the next period, or null for all time. */
    fun nextStartDay(period: StatsPeriod, weekStart: DayOfWeek, today: LocalDate): LocalDate? = when (period) {
        StatsPeriod.TODAY -> today.plusDays(1)
        StatsPeriod.WEEK -> today.with(TemporalAdjusters.next(weekStart))
        StatsPeriod.MONTH -> today.withDayOfMonth(1).plusMonths(1)
        StatsPeriod.ALL -> null
    }

    /** The week start before the user picks one: the locale's first day of the week. */
    fun defaultWeekStart(locale: Locale): DayOfWeek = WeekFields.of(locale).firstDayOfWeek

    /** Local midnight of [day] in epoch milliseconds; the start of the day across a DST change. */
    fun millis(day: LocalDate, zone: ZoneId): Long = day.atStartOfDay(zone).toInstant().toEpochMilli()
}

/** The gateway notification's text (spec 016). */
object NotificationText {
    fun counts(period: StatsPeriod, c: Counts, outbox: Int): String =
        "${period.label}: ${c.sent} sent, ${c.received} received" + if (outbox > 0) ", $outbox queued" else ""

    /** The collapsed line, and the expanded text when it differs: while reconnecting the error leads. */
    fun body(connected: Boolean, period: StatsPeriod, c: Counts, outbox: Int, error: String?): Pair<String, String?> {
        val line = counts(period, c, outbox)
        if (connected || error == null) return line to null
        return error to "$error\n$line"
    }
}

/**
 * The counts the notification shows (spec 016): the server's reply for the current period plus the
 * phone's own counts since the request, or the phone's own counts alone without a reply.
 * Used from the service's event loop only.
 */
class StatsTracker(private val daily: DailyStore) {
    private class Request(val id: String, val since: Long?, val local: Counts)

    private class Reply(val since: Long?, val server: Counts, val local: Counts)

    private var pending: Request? = null
    @Volatile
    private var reply: Reply? = null

    /** The server nacked `stats_req` as unsupported; no more requests until the next `welcome`. */
    var unsupported = false
        private set

    /** A request is due: after `welcome`, a settings change, a period rollover or a health. */
    var wanted = false

    /** Clears per-connection state at `welcome`; the last reply stays until the period moves on. */
    fun onWelcome() {
        pending = null
        unsupported = false
        wanted = true
    }

    /** Whether to send a request now: one is due, none is in flight, and the outbox is empty. */
    fun ready(welcomed: Boolean, outbox: Int): Boolean = welcomed && wanted && !unsupported && pending == null && outbox == 0

    /** Records a request sent as frame [id]; the phone's counts now are the base for later events. */
    fun requested(id: String, since: Long?, fromDay: Long?) {
        pending = Request(id, since, daily.sumDaily(fromDay))
        wanted = false
    }

    fun onStats(since: Long?, sent: Int, received: Int) {
        val req = pending?.takeIf { it.since == since } ?: return
        pending = null
        reply = Reply(since, Counts(sent, received), req.local)
    }

    /** A request was nacked; an `unsupported` one stops requests until the next `welcome`. */
    fun onNack(ref: String, code: String): Boolean {
        if (pending?.id != ref) return false
        pending = null
        if (code == "unsupported") unsupported = true
        return true
    }

    /** The connection dropped; a request in flight will not be answered. */
    fun onClosed() {
        pending = null
    }

    fun shown(since: Long?, fromDay: Long?): Counts {
        val local = daily.sumDaily(fromDay)
        val r = reply?.takeIf { it.since == since } ?: return local
        return r.server + (local - r.local)
    }
}
