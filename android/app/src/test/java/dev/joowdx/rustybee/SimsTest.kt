package dev.joowdx.rustybee

import android.telephony.ServiceState
import android.telephony.TelephonyManager
import dev.joowdx.rustybee.gateway.Telemetry
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** AC-002-23: a SIM turned off is not reported. */
class SimsTest {
    @Test
    fun ac_002_23_ready_sim_in_service_is_on() {
        assertTrue(Telemetry.simOn(TelephonyManager.SIM_STATE_READY, ServiceState.STATE_IN_SERVICE, airplane = false))
        assertTrue(Telemetry.simOn(TelephonyManager.SIM_STATE_READY, ServiceState.STATE_OUT_OF_SERVICE, airplane = false))
        assertTrue(Telemetry.simOn(TelephonyManager.SIM_STATE_READY, null, airplane = false))
    }

    @Test
    fun ac_002_23_unusable_card_is_off() {
        for (state in listOf(
            TelephonyManager.SIM_STATE_ABSENT,
            TelephonyManager.SIM_STATE_NOT_READY,
            TelephonyManager.SIM_STATE_PERM_DISABLED,
            TelephonyManager.SIM_STATE_CARD_IO_ERROR,
            TelephonyManager.SIM_STATE_CARD_RESTRICTED,
        )) {
            assertFalse("$state", Telemetry.simOn(state, ServiceState.STATE_IN_SERVICE, airplane = false))
        }
    }

    @Test
    fun ac_002_23_radio_off_is_off_except_in_airplane_mode() {
        assertFalse(Telemetry.simOn(TelephonyManager.SIM_STATE_READY, ServiceState.STATE_POWER_OFF, airplane = false))
        assertTrue(Telemetry.simOn(TelephonyManager.SIM_STATE_READY, ServiceState.STATE_POWER_OFF, airplane = true))
    }
}
