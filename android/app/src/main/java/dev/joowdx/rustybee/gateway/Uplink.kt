package dev.joowdx.rustybee.gateway

import dev.joowdx.rustybee.protocol.FrameType
import dev.joowdx.rustybee.protocol.Frames
import dev.joowdx.rustybee.protocol.Inbound
import dev.joowdx.rustybee.protocol.Status
import dev.joowdx.rustybee.protocol.Ulid

/**
 * Durable device→server frames (spec 011): every `status` and `inbound` goes into the outbox
 * first, is sent while a welcomed socket is attached, and is deleted when the server acks it.
 */
class Uplink(private val store: OutboxStore, private val clock: () -> Long = System::currentTimeMillis) {
    @Volatile
    private var sink: FrameSink? = null

    /** Called after `welcome`: attaches the socket and resends the outbox in `seq` order. */
    fun attach(sink: FrameSink) {
        this.sink = sink
        for (row in store.pending()) {
            if (!sink.send(row.json)) break
        }
    }

    fun detach() {
        sink = null
    }

    fun enqueueStatus(status: Status): String {
        val id = Ulid.next(clock())
        enqueue(id, FrameType.STATUS, Frames.encode(FrameType.STATUS, Status.serializer(), status, id))
        return id
    }

    /** Returns false (and queues nothing) when the same uid is already waiting in the outbox. */
    fun enqueueInbound(inbound: Inbound): Boolean {
        if (store.hasInbound(inbound.uid)) return false
        val id = Ulid.next(clock())
        enqueue(id, FrameType.INBOUND, Frames.encode(FrameType.INBOUND, Inbound.serializer(), inbound, id))
        return true
    }

    private fun enqueue(id: String, type: String, json: String) {
        if (store.insert(id, type, json, clock())) sink?.send(json)
    }

    /** An ack (or nack: never retransmit) for [ref] removes exactly that row. */
    fun onAck(ref: String): Boolean = store.delete(ref)

    fun size(): Int = store.count()
}
