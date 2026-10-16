package dev.joowdx.rustybee.gateway

import dev.joowdx.rustybee.protocol.Ack
import dev.joowdx.rustybee.protocol.Config
import dev.joowdx.rustybee.protocol.FrameType
import dev.joowdx.rustybee.protocol.Frames
import dev.joowdx.rustybee.protocol.Nack
import dev.joowdx.rustybee.protocol.PROTOCOL_VERSION
import dev.joowdx.rustybee.protocol.ProtocolJson
import dev.joowdx.rustybee.protocol.Send
import dev.joowdx.rustybee.protocol.Welcome
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonObject

/**
 * Handles one server→device text frame (spec 002): validate, dedupe by id, act, then ack.
 * [reply] writes directly to the current socket (acks and nacks never go through the outbox).
 */
class FrameHandler(
    private val seen: SeenStore,
    private val uplink: Uplink,
    private val sender: Sender,
    private val onWelcome: (Welcome) -> Unit,
    private val onConfig: (Config) -> Unit,
    private val clock: () -> Long = System::currentTimeMillis,
    private val log: (String) -> Unit = {},
) {
    fun handle(text: String, reply: (String) -> Unit) {
        val obj = try {
            ProtocolJson.parseToJsonElement(text).jsonObject
        } catch (e: Exception) {
            log("ignored malformed frame")
            return
        }
        val id = (obj["id"] as? JsonPrimitive)?.contentOrNull
        fun nack(code: String, message: String) {
            log("nack $code: $message")
            if (id != null) reply(Frames.nack(id, code, message))
        }
        if (id == null) return nack("bad_frame", "missing id")
        val v = (obj["v"] as? JsonPrimitive)?.intOrNull
        if (v != PROTOCOL_VERSION) return nack("unsupported", "unsupported version $v")
        val type = (obj["type"] as? JsonPrimitive)?.contentOrNull ?: return nack("bad_frame", "missing type")
        val data = obj["data"] ?: JsonObject(emptyMap())

        try {
            when (type) {
                FrameType.ACK -> uplink.onAck(ProtocolJson.decodeFromJsonElement(Ack.serializer(), data).ref)
                FrameType.NACK -> {
                    val n = ProtocolJson.decodeFromJsonElement(Nack.serializer(), data)
                    log("server nacked ${n.ref}: ${n.code} ${n.message ?: ""}")
                    uplink.onAck(n.ref)
                }
                FrameType.WELCOME -> {
                    val w = ProtocolJson.decodeFromJsonElement(Welcome.serializer(), data)
                    reply(Frames.ack(id))
                    onWelcome(w)
                }
                FrameType.CONFIG -> {
                    val c = ProtocolJson.decodeFromJsonElement(Config.serializer(), data)
                    onConfig(c)
                    reply(Frames.ack(id))
                }
                FrameType.SEND -> {
                    val s = ProtocolJson.decodeFromJsonElement(Send.serializer(), data)
                    if (seen.seen(id)) {
                        reply(Frames.ack(id))
                    } else {
                        sender.onSend(s) {
                            seen.markSeen(id, clock())
                            reply(Frames.ack(id))
                        }
                    }
                }
                else -> nack("unsupported", "unsupported type $type")
            }
        } catch (e: IllegalArgumentException) {
            nack("invalid", e.message?.take(200) ?: "invalid data")
        }
    }
}
