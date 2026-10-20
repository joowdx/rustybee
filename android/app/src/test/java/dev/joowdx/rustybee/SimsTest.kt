package dev.joowdx.rustybee

import android.telephony.ServiceState
import android.telephony.TelephonyManager
import dev.joowdx.rustybee.gateway.Telemetry
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** AC-002-23: a SIM turned off is not reported. */
class SimsTest {
    private val ready = TelephonyManager.SIM_STATE_READY

    @Test
    fun ac_002_23_ready_enabled_sim_is_on() {
        assertTrue(Telemetry.simOn(ready, ServiceState.STATE_IN_SERVICE, airplane = false))
        assertTrue(Telemetry.simOn(ready, ServiceState.STATE_OUT_OF_SERVICE, airplane = false))
        assertTrue(Telemetry.simOn(ready, null, airplane = false))
    }

    @Test
    fun ac_002_23_sim_turned_off_in_settings_is_off() {
        // As read on a Samsung A56 with its SIM turned off: still active, SIM state 16, out of service.
        assertFalse(Telemetry.simOn(16, ServiceState.STATE_OUT_OF_SERVICE, airplane = false))
    }

    @Test
    fun ac_002_23_card_not_ready_is_off() {
        for (state in listOf(
            TelephonyManager.SIM_STATE_ABSENT,
            TelephonyManager.SIM_STATE_NOT_READY,
            TelephonyManager.SIM_STATE_PIN_REQUIRED,
            TelephonyManager.SIM_STATE_PERM_DISABLED,
            TelephonyManager.SIM_STATE_CARD_IO_ERROR,
            TelephonyManager.SIM_STATE_UNKNOWN,
            16,
        )) {
            assertFalse("$state", Telemetry.simOn(state, ServiceState.STATE_IN_SERVICE, airplane = false))
        }
    }

    @Test
    fun ac_002_23_radio_off_is_off_except_in_airplane_mode() {
        assertFalse(Telemetry.simOn(ready, ServiceState.STATE_POWER_OFF, airplane = false))
        assertTrue(Telemetry.simOn(ready, ServiceState.STATE_POWER_OFF, airplane = true))
    }
}
