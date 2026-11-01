package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.StatsPeriod
import dev.joowdx.rustybee.gateway.StatsPeriods
import java.time.DayOfWeek
import java.time.LocalDate
import java.time.ZoneId
import java.time.ZonedDateTime
import java.util.Locale
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Spec 016 periods: AC-016-04 and AC-016-05. */
class StatsPeriodTest {
    private val wed = LocalDate.of(2026, 11, 4) // a Wednesday

    @Test
    fun ac_016_04_defaults_are_today_and_the_locale_week_start() {
        assertEquals(StatsPeriod.TODAY, StatsPeriod.of(null))
        assertEquals(StatsPeriod.TODAY, StatsPeriod.of("bogus"))
        assertEquals(StatsPeriod.WEEK, StatsPeriod.of("week"))
        assertEquals(DayOfWeek.SUNDAY, StatsPeriods.defaultWeekStart(Locale.US))
        assertEquals(DayOfWeek.MONDAY, StatsPeriods.defaultWeekStart(Locale.GERMANY))
    }

    @Test
    fun ac_016_05_period_starts() {
        assertEquals(wed, StatsPeriods.startDay(StatsPeriod.TODAY, DayOfWeek.MONDAY, wed))
        assertEquals(LocalDate.of(2026, 11, 2), StatsPeriods.startDay(StatsPeriod.WEEK, DayOfWeek.MONDAY, wed))
        assertEquals(LocalDate.of(2026, 11, 1), StatsPeriods.startDay(StatsPeriod.WEEK, DayOfWeek.SUNDAY, wed))
        assertEquals(LocalDate.of(2026, 10, 29), StatsPeriods.startDay(StatsPeriod.WEEK, DayOfWeek.THURSDAY, wed))
        // On the start day itself the week starts today.
        assertEquals(wed, StatsPeriods.startDay(StatsPeriod.WEEK, DayOfWeek.WEDNESDAY, wed))
        assertEquals(LocalDate.of(2026, 11, 1), StatsPeriods.startDay(StatsPeriod.MONTH, DayOfWeek.MONDAY, wed))
        assertNull(StatsPeriods.startDay(StatsPeriod.ALL, DayOfWeek.MONDAY, wed))
    }

    @Test
    fun ac_016_05_next_period_starts() {
        assertEquals(wed.plusDays(1), StatsPeriods.nextStartDay(StatsPeriod.TODAY, DayOfWeek.MONDAY, wed))
        assertEquals(LocalDate.of(2026, 11, 9), StatsPeriods.nextStartDay(StatsPeriod.WEEK, DayOfWeek.MONDAY, wed))
        assertEquals(LocalDate.of(2026, 11, 11), StatsPeriods.nextStartDay(StatsPeriod.WEEK, DayOfWeek.WEDNESDAY, wed))
        assertEquals(LocalDate.of(2026, 12, 1), StatsPeriods.nextStartDay(StatsPeriod.MONTH, DayOfWeek.MONDAY, wed))
        assertNull(StatsPeriods.nextStartDay(StatsPeriod.ALL, DayOfWeek.MONDAY, wed))
    }

    @Test
    fun ac_016_05_midnight_across_dst() {
        val ny = ZoneId.of("America/New_York")
        // 2026-03-08 springs forward at 02:00; midnight still exists and the day is 23 hours long.
        val day = LocalDate.of(2026, 3, 8)
        assertEquals(ZonedDateTime.of(2026, 3, 8, 0, 0, 0, 0, ny).toInstant().toEpochMilli(), StatsPeriods.millis(day, ny))
        assertEquals(23 * 3_600_000L, StatsPeriods.millis(day.plusDays(1), ny) - StatsPeriods.millis(day, ny))
        // Where DST starts at midnight, the day starts at 01:00.
        val havana = ZoneId.of("America/Havana")
        val shift = LocalDate.of(2026, 3, 8)
        assertEquals(1, java.time.Instant.ofEpochMilli(StatsPeriods.millis(shift, havana)).atZone(havana).hour)
    }
}
