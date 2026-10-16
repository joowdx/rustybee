package dev.joowdx.rustybee.gateway

import dev.joowdx.rustybee.protocol.Status
import dev.joowdx.rustybee.protocol.StatusError

/** Pure combination of per-part results into one message outcome (spec 012 §6). */
object PartAggregator {
    data class Result(val row: SentRow, val statuses: List<Status>)

    fun status(row: SentRow, at: Long): Status = Status(
        msgId = row.msgId,
        state = row.state,
        at = at,
        parts = row.parts,
        error = row.errorCode?.let { StatusError(it, row.errorMessage ?: "") },
    )

    /** One part's sent callback. [resultCode] is `RESULT_OK` (-1) on success. */
    fun onSent(row: SentRow, resultCode: Int, now: Long): Result {
        if (row.state != SentState.ACCEPTED) return Result(row, emptyList())
        var next = if (resultCode == SmsErrors.RESULT_OK) {
            row.copy(partsSent = row.partsSent + 1, updatedAt = now)
        } else {
            val err = if (row.errorCode == null) SmsErrors.fromResult(resultCode) else null
            row.copy(
                partsFailed = row.partsFailed + 1,
                errorCode = err?.code ?: row.errorCode,
                errorMessage = err?.message ?: row.errorMessage,
                updatedAt = now,
            )
        }
        if (next.partsSent + next.partsFailed < next.parts) return Result(next, emptyList())
        return if (next.partsFailed > 0) {
            next = next.copy(state = SentState.FAILED)
            Result(next, listOf(status(next, now)))
        } else {
            next = next.copy(state = SentState.SENT)
            val out = mutableListOf(status(next, now))
            if (next.partsDelivered >= next.parts) {
                next = next.copy(state = SentState.DELIVERED)
                out += status(next, now)
            }
            Result(next, out)
        }
    }

    /** One part's delivery report. [pduStatus] is the TP-Status, or null if no PDU came back. */
    fun onDelivered(row: SentRow, pduStatus: Int?, now: Long): Result {
        if (row.state == SentState.FAILED || row.state == SentState.DELIVERED) return Result(row, emptyList())
        val status = pduStatus ?: 0
        return when {
            status >= 0x40 -> {
                val err = SmsErrors.deliveryFailed
                val next = row.copy(state = SentState.FAILED, errorCode = err.code, errorMessage = err.message, updatedAt = now)
                Result(next, listOf(status(next, now)))
            }
            status >= 0x20 -> Result(row, emptyList())
            else -> {
                var next = row.copy(partsDelivered = row.partsDelivered + 1, updatedAt = now)
                if (next.state == SentState.SENT && next.partsDelivered >= next.parts) {
                    next = next.copy(state = SentState.DELIVERED)
                    Result(next, listOf(status(next, now)))
                } else {
                    Result(next, emptyList())
                }
            }
        }
    }

    /** Statuses to re-report for a duplicate `send` whose outcome is already known. */
    fun replay(row: SentRow, now: Long): List<Status> = when (row.state) {
        SentState.SENT, SentState.FAILED -> listOf(status(row, now))
        SentState.DELIVERED -> listOf(status(row.copy(state = SentState.SENT), now), status(row, now))
        else -> emptyList()
    }
}
