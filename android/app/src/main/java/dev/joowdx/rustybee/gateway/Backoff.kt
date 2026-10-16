package dev.joowdx.rustybee.gateway

import kotlin.math.min

/**
 * Spec 002 reconnect backoff: 1 s doubling to 120 s with ±20 % jitter. A connection that stayed
 * up for at least [stableMs] resets the sequence to 1 s.
 */
class Backoff(
    private val random: () -> Double = Math::random,
    private val baseMs: Long = 1_000,
    private val maxMs: Long = 120_000,
    private val jitter: Double = 0.2,
    private val stableMs: Long = 60_000,
) {
    private var attempt = 0
    private var openedAt: Long? = null

    /** Delay before the next reconnect attempt. */
    fun nextDelayMs(): Long {
        val nominal = min(maxMs, baseMs shl min(attempt, 20))
        if (nominal < maxMs) attempt++
        val factor = 1.0 + jitter * (2.0 * random() - 1.0)
        return (nominal * factor).toLong()
    }

    fun onOpened(now: Long) {
        openedAt = now
    }

    fun onClosed(now: Long) {
        val opened = openedAt ?: return
        if (now - opened >= stableMs) attempt = 0
        openedAt = null
    }

    fun reset() {
        attempt = 0
        openedAt = null
    }
}
