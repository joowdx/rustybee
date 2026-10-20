package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.TelephonyWatch.Watch
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * AC-011-13: on Android 11 each watch listens for the same events as its TelephonyCallback.
 * The flags are PhoneStateListener's published values, written out so a wrong constant shows.
 */
class TelephonyWatchTest {
    private val serviceState = 0x1
    private val signalStrengths = 0x100
    private val userMobileData = 0x80000
    private val displayInfo = 0x100000

    @Test
    fun ac_011_13_android_11_listener_flags() {
        assertEquals(serviceState, Watch.SERVICE_STATE.listenEvents)
        assertEquals(serviceState or signalStrengths, Watch.SERVICE_STATE_AND_SIGNAL.listenEvents)
        assertEquals(displayInfo or userMobileData, Watch.DISPLAY_INFO_AND_MOBILE_DATA.listenEvents)
    }
}
