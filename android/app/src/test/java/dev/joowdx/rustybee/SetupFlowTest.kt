package dev.joowdx.rustybee

import dev.joowdx.rustybee.ui.PairConfirm
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** AC-013-07. */
class SetupFlowTest {
    @Test
    fun ac_013_07_deep_link_fills_the_field_but_does_not_pair() {
        val link = "rustybee://pair?u=https://sms.example.com&c=ABC123"
        assertEquals(link, PairConfirm.linkFromIntent(link))
        assertFalse(PairConfirm.AUTO_PAIR_FROM_INTENT)
    }

    @Test
    fun ac_013_07_other_intents_are_ignored() {
        assertNull(PairConfirm.linkFromIntent(null))
        assertNull(PairConfirm.linkFromIntent("https://sms.example.com"))
    }

    @Test
    fun ac_013_07_confirm_names_the_host() {
        val m = PairConfirm.message("sms.example.com", paired = false)
        assertTrue(m.contains("sms.example.com"))
        assertFalse(m.contains("replaces"))
    }

    @Test
    fun ac_013_07_confirm_when_paired_says_it_replaces_and_wipes() {
        val m = PairConfirm.message("other.example.com", paired = true)
        assertTrue(m.contains("other.example.com"))
        assertTrue(m.contains("replaces the current server"))
        assertTrue(m.contains("wipes"))
    }
}
