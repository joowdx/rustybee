package dev.joowdx.rustybee.gateway

import dev.joowdx.rustybee.protocol.Send
import dev.joowdx.rustybee.protocol.StatusError

/** The radio, behind a seam so dedupe and aggregation stay JVM-testable. */
interface SmsRadio {
    fun activeSubscriptionIds(): Set<Int>
    fun divide(body: String, sim: Int?): List<String>

    /** Hands all parts to SmsManager. May throw SecurityException or IllegalArgumentException. */
    fun send(msgId: String, to: String, parts: List<String>, sim: Int?)
}

/** Spec 012 send path: dedupe on `msg_id`, ack, hand to the radio, combine part results. */
class Sender(
    private val sent: SentStore,
    private val uplink: Uplink,
    private val radio: SmsRadio,
    private val configSim: () -> Int?,
    private val clock: () -> Long = System::currentTimeMillis,
    private val log: (String) -> Unit = {},
) {
    /** Handles one `send` frame. [ack] is invoked once the row is durable, before the radio call. */
    fun onSend(send: Send, ack: () -> Unit) {
        val now = clock()
        val existing = sent.get(send.msgId)
        if (existing != null) {
            if (existing.state != SentState.ACCEPTED) {
                PartAggregator.replay(existing, now).forEach(uplink::enqueueStatus)
            }
            ack()
            return
        }

        val active = radio.activeSubscriptionIds()
        val sim: Int? = when {
            send.sim != null -> if (send.sim in active) send.sim else null
            else -> configSim()?.takeIf { it in active }
        }
        if (send.sim != null && sim == null) {
            fail(SentRow(send.msgId, send.to, SentState.ACCEPTED, 1, updatedAt = now), SmsErrors.simUnavailable, insert = true)
            ack()
            return
        }

        val parts = try {
            radio.divide(send.body, sim).ifEmpty { listOf(send.body) }
        } catch (e: RuntimeException) {
            listOf(send.body)
        }
        val row = SentRow(send.msgId, send.to, SentState.ACCEPTED, parts.size, updatedAt = now)
        sent.insert(row)
        ack()
        try {
            radio.send(send.msgId, send.to, parts, sim)
            log("send ${send.msgId} to ${Redact.phone(send.to)}: ${parts.size} part(s) handed to radio")
        } catch (e: SecurityException) {
            fail(row, SmsErrors.permissionDenied, insert = false)
        } catch (e: IllegalArgumentException) {
            fail(row, SmsErrors.invalidDestination, insert = false)
        }
    }

    fun onSentResult(msgId: String, resultCode: Int) = apply(msgId) { PartAggregator.onSent(it, resultCode, clock()) }

    fun onDeliveredResult(msgId: String, pduStatus: Int?) =
        apply(msgId) { PartAggregator.onDelivered(it, pduStatus, clock()) }

    /** Spec 012 §8: rows left `accepted` for 10 min after a process death are reported failed. */
    fun failLost(maxAgeMs: Long = 10 * 60_000L) {
        sent.acceptedBefore(clock() - maxAgeMs).forEach { fail(it, SmsErrors.outcomeLost, insert = false) }
    }

    private fun apply(msgId: String, step: (SentRow) -> PartAggregator.Result) {
        val statuses = sent.atomically {
            val row = sent.get(msgId) ?: return@atomically emptyList()
            val result = step(row)
            if (result.row != row) sent.update(result.row)
            result.statuses
        }
        statuses.forEach {
            log("status ${it.msgId} ${it.state}${it.error?.let { e -> " (${e.code})" } ?: ""}")
            uplink.enqueueStatus(it)
        }
    }

    private fun fail(row: SentRow, error: StatusError, insert: Boolean) {
        val now = clock()
        val failed = row.copy(state = SentState.FAILED, errorCode = error.code, errorMessage = error.message, updatedAt = now)
        if (insert) sent.insert(failed) else sent.update(failed)
        log("status ${row.msgId} failed (${error.code})")
        uplink.enqueueStatus(PartAggregator.status(failed, now))
    }
}
