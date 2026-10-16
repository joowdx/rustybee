package dev.joowdx.rustybee.gateway

/** Storage seams implemented by `data.Db`; faked in JVM tests. */

data class OutboxRow(val seq: Long, val frameId: String, val type: String, val json: String, val createdAt: Long)

interface OutboxStore {
    /** Inserts a frame; returns false if `frame_id` already exists. */
    fun insert(frameId: String, type: String, json: String, createdAt: Long): Boolean

    /** True if an `inbound` frame with this uid is still queued. */
    fun hasInbound(uid: String): Boolean

    /** All queued frames in `seq` order. */
    fun pending(): List<OutboxRow>

    /** Deletes the row with this frame id; returns true if a row was removed. */
    fun delete(frameId: String): Boolean

    fun count(): Int
}

object SentState {
    const val ACCEPTED = "accepted"
    const val SENT = "sent"
    const val DELIVERED = "delivered"
    const val FAILED = "failed"

    /** Terminal states; rows in them are pruned after 7 days (AC-011-08). */
    val FINAL = listOf(SENT, DELIVERED, FAILED)
}

data class SentRow(
    val msgId: String,
    val to: String,
    val state: String,
    val parts: Int,
    val partsSent: Int = 0,
    val partsFailed: Int = 0,
    val partsDelivered: Int = 0,
    val errorCode: String? = null,
    val errorMessage: String? = null,
    val updatedAt: Long,
)

interface SentStore {
    fun get(msgId: String): SentRow?
    fun insert(row: SentRow)
    fun update(row: SentRow)

    /** Rows still `accepted` whose `updated_at` is before [before]. */
    fun acceptedBefore(before: Long): List<SentRow>

    /** Runs [block] atomically with respect to other sent-table writers. */
    fun <T> atomically(block: () -> T): T
}

interface SeenStore {
    fun seen(frameId: String): Boolean
    fun markSeen(frameId: String, at: Long)
}

/** A connected, welcomed socket that can take text frames. */
fun interface FrameSink {
    fun send(text: String): Boolean
}
