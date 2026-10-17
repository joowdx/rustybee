package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.SimChoice
import dev.joowdx.rustybee.gateway.SimChoice.Sub
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** AC-012-09: the subscription used when neither `sim` nor `config.sim` picks one. */
class SimChoiceTest {
    @Test
    fun ac_012_09_system_default_is_used_when_set() {
        assertNull(SimChoice.fallback(3, listOf(Sub(3, 0), Sub(5, 1))))
    }

    @Test
    fun ac_012_09_ask_every_time_uses_the_single_active_sim() {
        assertEquals(5, SimChoice.fallback(SimChoice.INVALID, listOf(Sub(5, 1))))
    }

    @Test
    fun ac_012_09_ask_every_time_uses_the_lowest_slot() {
        assertEquals(7, SimChoice.fallback(SimChoice.INVALID, listOf(Sub(9, 1), Sub(7, 0))))
    }

    @Test
    fun ac_012_09_no_active_sim_leaves_the_system_default() {
        assertNull(SimChoice.fallback(SimChoice.INVALID, emptyList()))
    }
}
