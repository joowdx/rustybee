package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.OutboxRow
import dev.joowdx.rustybee.gateway.OutboxStore
import dev.joowdx.rustybee.gateway.SeenStore
import dev.joowdx.rustybee.gateway.SentRow
import dev.joowdx.rustybee.gateway.SentState
import dev.joowdx.rustybee.gateway.SentStore
import dev.joowdx.rustybee.gateway.SmsRadio
import java.io.File

class FakeOutbox : OutboxStore {
    private var seq = 0L
    val rows = mutableListOf<OutboxRow>()

    override fun insert(frameId: String, type: String, json: String, createdAt: Long): Boolean {
        if (rows.any { it.frameId == frameId }) return false
        rows += OutboxRow(++seq, frameId, type, json, createdAt)
        return true
    }

    override fun hasInbound(uid: String) = rows.any { it.type == "inbound" && it.json.contains("\"uid\":\"$uid\"") }
    override fun pending() = rows.sortedBy { it.seq }
    override fun delete(frameId: String) = rows.removeIf { it.frameId == frameId }
    override fun count() = rows.size
}

class FakeSent : SentStore {
    val rows = linkedMapOf<String, SentRow>()
    override fun get(msgId: String) = rows[msgId]
    override fun insert(row: SentRow) {
        rows.putIfAbsent(row.msgId, row)
    }
    override fun update(row: SentRow) {
        rows[row.msgId] = row
    }
    override fun acceptedBefore(before: Long) = rows.values.filter { it.state == SentState.ACCEPTED && it.updatedAt < before }
    override fun <T> atomically(block: () -> T): T = block()
}

class FakeSeen : SeenStore {
    val ids = mutableMapOf<String, Long>()
    override fun seen(frameId: String) = frameId in ids
    override fun markSeen(frameId: String, at: Long) {
        ids[frameId] = at
    }
}

class FakeRadio(
    var active: Set<Int> = setOf(1, 2),
    var partsPerMessage: Int = 1,
    var throwOnSend: RuntimeException? = null,
    var readSims: Boolean = true,
) : SmsRadio {
    data class Call(val msgId: String, val to: String, val parts: List<String>, val sim: Int?)

    val calls = mutableListOf<Call>()
    override fun canReadSims() = readSims
    override fun activeSubscriptionIds() = if (readSims) active else emptySet()
    override fun divide(body: String, sim: Int?) = List(partsPerMessage) { "$body#$it" }
    override fun send(msgId: String, to: String, parts: List<String>, sim: Int?) {
        throwOnSend?.let { throw it }
        calls += Call(msgId, to, parts, sim)
    }
}

/** The shared fixtures directory, on the test classpath via a resources srcDir. */
object Fixtures {
    val dir: File by lazy {
        val url = checkNotNull(Fixtures::class.java.classLoader!!.getResource("hello.json")) { "protocol fixtures missing" }
        File(url.toURI()).parentFile
    }

    fun read(name: String): String = File(dir, name).readText()
}
