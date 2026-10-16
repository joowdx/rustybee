package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.Forwarding
import dev.joowdx.rustybee.protocol.Config
import dev.joowdx.rustybee.protocol.Filters
import dev.joowdx.rustybee.protocol.SetForwarding
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ForwardingTest {
    private val filters = Filters("block")

    @Test
    fun ac_002_12_local_off_wins_over_server_on() {
        val on = Config(receiveEnabled = true, filters = filters, filtersRev = 4)
        assertFalse(Forwarding.on(on, offHere = true))
        assertEquals(SetForwarding(4, false, filters), Forwarding.restore(on, offHere = true))
    }

    @Test
    fun server_off_stops_forwarding_without_a_restore() {
        val off = Config(receiveEnabled = false)
        assertFalse(Forwarding.on(off, offHere = false))
        assertFalse(Forwarding.on(off, offHere = true))
        assertNull(Forwarding.restore(off, offHere = true))
    }

    @Test
    fun forwards_when_both_sides_agree_on() {
        val on = Config(receiveEnabled = true)
        assertTrue(Forwarding.on(on, offHere = false))
        assertNull(Forwarding.restore(on, offHere = false))
    }
}
