package dev.joowdx.rustybee.gateway

/** Spec 012 §2: which subscription a send without `sim` or `config.sim` uses. Pure, for JVM tests. */
object SimChoice {
    /** `SubscriptionManager.INVALID_SUBSCRIPTION_ID`. */
    const val INVALID = -1

    data class Sub(val id: Int, val slot: Int)

    /**
     * Null means "use the system default SmsManager". When there is no default SMS subscription
     * ("Ask every time" on dual-SIM), the single active subscription is used, or else the one in the lowest slot.
     */
    fun fallback(defaultSmsId: Int, active: List<Sub>): Int? {
        if (defaultSmsId != INVALID) return null
        active.singleOrNull()?.let { return it.id }
        return active.minWithOrNull(compareBy({ it.slot }, { it.id }))?.id
    }
}
