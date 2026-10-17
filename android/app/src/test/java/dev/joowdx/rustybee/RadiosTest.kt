package dev.joowdx.rustybee

import android.telephony.TelephonyDisplayInfo
import android.telephony.TelephonyManager
import dev.joowdx.rustybee.gateway.RadioDecision
import dev.joowdx.rustybee.gateway.RadioReading
import dev.joowdx.rustybee.gateway.RadioReport
import dev.joowdx.rustybee.gateway.RadioSwitches
import dev.joowdx.rustybee.protocol.Health
import dev.joowdx.rustybee.protocol.ProtocolJson
import dev.joowdx.rustybee.protocol.Radio
import dev.joowdx.rustybee.protocol.Radios
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Network details (spec 015): AC-015-05 to AC-015-08 and AC-015-10. */
class RadiosTest {
    private val cell = RadioReading("Example Mobile", 3, -95)
    private val wifi = RadioReading("HomeNet", 4, -55)

    @Test
    fun ac_015_05_switches_default_off_and_report_empty() {
        val off = RadioSwitches()
        assertEquals(RadioSwitches(false, false, false, false), off)
        val radios = RadioReport.build(off, cell, wifi)
        assertEquals(Radios(), radios)
        assertEquals("{}", ProtocolJson.encodeToString(Radios.serializer(), radios))
    }

    @Test
    fun ac_015_06_only_switched_on_fields() {
        val nameOnly = RadioReport.build(RadioSwitches(cellularName = true), cell, wifi)
        assertEquals(Radios(cellular = Radio(name = "Example Mobile")), nameOnly)
        val signalOnly = RadioReport.build(RadioSwitches(wifiSignal = true), cell, wifi)
        assertEquals(Radios(wifi = Radio(level = 4, dbm = -55)), signalOnly)
        // Nothing readable for a switched-on radio leaves it out.
        assertEquals(Radios(), RadioReport.build(RadioSwitches(wifiName = true), cell, RadioReading(level = 2)))
        assertEquals(Radios(), RadioReport.build(RadioSwitches(cellularName = true), null, wifi))
        val health = Health(
            battery = dev.joowdx.rustybee.protocol.Battery(50, false),
            network = "wifi",
            permissions = dev.joowdx.rustybee.protocol.Permissions(true, true, true, true),
            outbox = 0,
            radios = nameOnly,
        )
        assertTrue(ProtocolJson.encodeToString(Health.serializer(), health).contains("\"radios\":{\"cellular\":{\"name\":\"Example Mobile\"}}"))
    }

    @Test
    fun ac_015_07_when_to_report() {
        val sw = RadioSwitches(cellularName = true, cellularSignal = true)
        val sent = RadioReport.build(sw, cell, null)
        fun decide(now: Radios, since: Long? = 5_000) = RadioReport.decide(true, sent, now, since)
        assertEquals(RadioDecision.Skip, decide(sent))
        // dBm alone never reports.
        assertEquals(RadioDecision.Skip, decide(RadioReport.build(sw, cell.copy(dbm = -97), null)))
        // A bar change waits out the minute, then goes.
        assertEquals(RadioDecision.Later(55_000), decide(RadioReport.build(sw, cell.copy(level = 2), null)))
        assertEquals(RadioDecision.Now, decide(RadioReport.build(sw, cell.copy(level = 2), null), since = 60_000))
        // Names and switches go at once.
        assertEquals(RadioDecision.Now, decide(RadioReport.build(sw, cell.copy(name = "Other"), null)))
        assertEquals(RadioDecision.Now, decide(RadioReport.build(RadioSwitches(cellularName = true), cell, null)))
        assertEquals(RadioDecision.Now, decide(RadioReport.build(sw.copy(wifiSignal = true), cell, wifi)))
        // Never before welcome, and the first report goes at once.
        assertEquals(RadioDecision.Skip, RadioReport.decide(false, sent, Radios(), 0))
        assertEquals(RadioDecision.Now, RadioReport.decide(true, null, sent, null))
    }

    @Test
    fun ac_015_11_wait_for_new_listener() {
        val sw = RadioSwitches(cellularName = true, cellularSignal = true)
        val sent = RadioReport.build(RadioSwitches(cellularName = true), cell, null)
        val now = RadioReport.build(sw, cell, null)
        // A switch change normally goes at once, but waits while a new listener settles.
        assertEquals(RadioDecision.Now, RadioReport.decide(true, sent, now, 5_000))
        assertEquals(RadioDecision.Later(1_500), RadioReport.decide(true, sent, now, 5_000, settlingMs = 1_500))
        // Nothing changed: nothing to wait for.
        assertEquals(RadioDecision.Skip, RadioReport.decide(true, sent, sent, 5_000, settlingMs = 1_500))
        // Never before welcome.
        assertEquals(RadioDecision.Skip, RadioReport.decide(false, sent, now, 5_000, settlingMs = 1_500))
    }

    @Test
    fun ac_015_08_wifi_name_cleanup() {
        assertEquals("HomeNet", RadioReport.ssid("\"HomeNet\""))
        assertNull(RadioReport.ssid("<unknown ssid>"))
        assertNull(RadioReport.ssid("\"\""))
        assertNull(RadioReport.ssid(null))
        assertEquals(4, RadioReport.bars(4, 4))
        assertEquals(0, RadioReport.bars(0, 4))
        assertEquals(2, RadioReport.bars(2, 4))
        assertEquals(2, RadioReport.bars(3, 5))
        assertEquals(3, RadioReport.bars(4, 5))
    }

    @Test
    fun ac_015_10_cellular_generation() {
        assertEquals("5G", RadioReport.tech(TelephonyManager.NETWORK_TYPE_NR))
        assertEquals("5G", RadioReport.tech(TelephonyManager.NETWORK_TYPE_LTE, TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_NR_NSA))
        assertEquals("4G", RadioReport.tech(TelephonyManager.NETWORK_TYPE_LTE, TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_LTE_CA))
        assertEquals("3G", RadioReport.tech(TelephonyManager.NETWORK_TYPE_HSPAP))
        assertEquals("2G", RadioReport.tech(TelephonyManager.NETWORK_TYPE_EDGE))
        assertNull(RadioReport.tech(TelephonyManager.NETWORK_TYPE_UNKNOWN))
        assertNull(RadioReport.tech(TelephonyManager.NETWORK_TYPE_IWLAN))

        // Sent with the signal switch only.
        val reading = RadioReading("Example Mobile", 3, -95, "5G")
        assertNull(RadioReport.build(RadioSwitches(cellularName = true), reading, null).cellular?.tech)
        assertEquals("5G", RadioReport.build(RadioSwitches(cellularSignal = true), reading, null).cellular?.tech)

        // A generation change waits like a bar change.
        val sent = Radios(cellular = Radio(level = 3, tech = "4G"))
        val now = Radios(cellular = Radio(level = 3, tech = "5G"))
        assertEquals(RadioDecision.Later(55_000), RadioReport.decide(true, sent, now, 5_000))
        assertEquals(RadioDecision.Now, RadioReport.decide(true, sent, now, 60_000))
        // Mobile data turning off drops the generation, and that reports at once.
        assertEquals(RadioDecision.Now, RadioReport.decide(true, sent, Radios(cellular = Radio(level = 3)), 5_000))
    }
}
