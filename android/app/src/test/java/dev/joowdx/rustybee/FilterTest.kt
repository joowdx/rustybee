package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.InboundFilter
import dev.joowdx.rustybee.protocol.FilterRule
import dev.joowdx.rustybee.protocol.Filters
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** AC-012-04: three modes × four operations. */
class FilterTest {
    private val from = "+15551234567"
    private val body = "Your PROMO code is 123456"

    private fun rule(op: String, value: String, field: String = "body", ic: Boolean = true) = FilterRule(field, op, value, ic)

    private val matching = listOf(
        rule("equals", "your promo code is 123456"),
        rule("contains", "promo"),
        rule("starts_with", "your"),
        rule("ends_with", "3456"),
    )
    private val nonMatching = listOf(
        rule("equals", "promo"),
        rule("contains", "discount"),
        rule("starts_with", "code"),
        rule("ends_with", "your"),
    )

    @Test
    fun ac_012_04_off_forwards_everything() {
        (matching + nonMatching).forEach {
            assertTrue(InboundFilter.forward(Filters("off", listOf(it)), from, body))
        }
        assertTrue(InboundFilter.forward(Filters(), from, body))
    }

    @Test
    fun ac_012_04_allow_forwards_only_matches() {
        matching.forEach { assertTrue(it.op, InboundFilter.forward(Filters("allow", listOf(it)), from, body)) }
        nonMatching.forEach { assertFalse(it.op, InboundFilter.forward(Filters("allow", listOf(it)), from, body)) }
        assertTrue(InboundFilter.forward(Filters("allow", nonMatching + matching[0]), from, body))
        assertFalse(InboundFilter.forward(Filters("allow", emptyList()), from, body))
    }

    @Test
    fun ac_012_04_block_drops_matches() {
        matching.forEach { assertFalse(it.op, InboundFilter.forward(Filters("block", listOf(it)), from, body)) }
        nonMatching.forEach { assertTrue(it.op, InboundFilter.forward(Filters("block", listOf(it)), from, body)) }
        assertTrue(InboundFilter.forward(Filters("block", emptyList()), from, body))
    }

    @Test
    fun ac_012_04_fields_and_case() {
        assertTrue(InboundFilter.matches(rule("starts_with", "+1555", field = "sender"), from, body))
        assertFalse(InboundFilter.matches(rule("starts_with", "+1555", field = "body"), from, body))
        assertTrue(InboundFilter.matches(rule("ends_with", "4567", field = "any"), from, body))
        assertTrue(InboundFilter.matches(rule("contains", "promo", field = "any"), from, body))
        assertFalse(InboundFilter.matches(rule("contains", "promo", ic = false), from, body))
        assertTrue(InboundFilter.matches(rule("contains", "PROMO", ic = false), from, body))
    }

    @Test
    fun ac_012_04_config_fixture_blocks_premium_and_promo() {
        val filters = Filters(
            "block",
            listOf(rule("starts_with", "+1900", field = "sender"), rule("contains", "promo")),
        )
        assertFalse(InboundFilter.forward(filters, "+19005550000", "hi"))
        assertFalse(InboundFilter.forward(filters, from, body))
        assertEquals(true, InboundFilter.forward(filters, from, "hello"))
    }
}
