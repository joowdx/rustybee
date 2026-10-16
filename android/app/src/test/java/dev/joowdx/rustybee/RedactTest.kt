package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.Redact
import dev.joowdx.rustybee.gateway.SentState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

/** AC-011-07, AC-011-08. */
class RedactTest {
    @Test
    fun ac_011_07_plain_and_dashed_numbers_keep_last_4() {
        assertEquals("sent to …4567", Redact.text("sent to +15551234567"))
        assertEquals("sent to …4567", Redact.text("sent to 555-123-4567"))
        assertEquals("sent to …4567", Redact.text("sent to +1 555 123 4567"))
    }

    @Test
    fun ac_011_07_dotted_and_parenthesised_numbers_keep_last_4() {
        assertEquals("sent to …4567", Redact.text("sent to 555.123.4567"))
        assertEquals("sent to …4567", Redact.text("sent to (555) 123-4567"))
        assertEquals("sent to …4567", Redact.text("sent to +1 (555) 123-4567"))
    }

    @Test
    fun ac_011_07_short_numbers_and_ids_are_left_alone() {
        assertEquals("3 parts, code 1234", Redact.text("3 parts, code 1234"))
        assertEquals("msg abc12345678 ok", Redact.text("msg abc12345678 ok"))
        assertEquals("…4567", Redact.phone("+1 (555) 123-4567"))
        assertEquals("1234", Redact.phone("1234"))
    }

    @Test
    fun ac_011_08_only_final_sent_states_are_pruned() {
        assertEquals(listOf("sent", "delivered", "failed"), SentState.FINAL)
        assertFalse(SentState.ACCEPTED in SentState.FINAL)
    }
}
