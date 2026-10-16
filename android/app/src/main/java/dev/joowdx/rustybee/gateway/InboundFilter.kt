package dev.joowdx.rustybee.gateway

import dev.joowdx.rustybee.protocol.FilterRule
import dev.joowdx.rustybee.protocol.Filters

/** Spec 002 filter semantics. */
object InboundFilter {
    fun forward(filters: Filters, from: String, body: String): Boolean = when (filters.mode) {
        "allow" -> filters.rules.any { matches(it, from, body) }
        "block" -> filters.rules.none { matches(it, from, body) }
        else -> true
    }

    fun matches(rule: FilterRule, from: String, body: String): Boolean = when (rule.field) {
        "sender" -> test(rule, from)
        "body" -> test(rule, body)
        "any" -> test(rule, from) || test(rule, body)
        else -> false
    }

    private fun test(rule: FilterRule, text: String): Boolean {
        val ic = rule.ignoreCase
        return when (rule.op) {
            "equals" -> text.equals(rule.value, ic)
            "contains" -> text.contains(rule.value, ic)
            "starts_with" -> text.startsWith(rule.value, ic)
            "ends_with" -> text.endsWith(rule.value, ic)
            else -> false
        }
    }
}
