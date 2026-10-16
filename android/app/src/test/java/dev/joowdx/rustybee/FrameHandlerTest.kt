package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.FrameHandler
import dev.joowdx.rustybee.gateway.Sender
import dev.joowdx.rustybee.gateway.Uplink
import dev.joowdx.rustybee.protocol.Envelope
import dev.joowdx.rustybee.protocol.Nack
import dev.joowdx.rustybee.protocol.ProtocolJson
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** App half of AC-002-08: bad frames with an id are nacked; frames without one are ignored. */
class FrameHandlerTest {
    private val replies = mutableListOf<String>()
    private val handler = FrameHandler(
        seen = FakeSeen(),
        uplink = Uplink(FakeOutbox()),
        sender = Sender(FakeSent(), Uplink(FakeOutbox()), FakeRadio(), { null }),
        onWelcome = {},
        onConfig = {},
    )

    private fun lastNack(): Nack {
        val env = ProtocolJson.decodeFromString(Envelope.serializer(), replies.last())
        assertEquals("nack", env.type)
        return ProtocolJson.decodeFromJsonElement(Nack.serializer(), env.data)
    }

    @Test
    fun ac_002_08_malformed_json_is_ignored() {
        handler.handle("{not json") { replies += it }
        assertTrue(replies.isEmpty())
    }

    @Test
    fun ac_002_08_wrong_version_unknown_type_and_bad_data_are_nacked() {
        handler.handle("""{"v":2,"id":"A1","type":"send","data":{}}""") { replies += it }
        assertEquals("unsupported", lastNack().code)
        handler.handle("""{"v":1,"id":"A2","type":"bogus","data":{}}""") { replies += it }
        assertEquals("unsupported", lastNack().code)
        handler.handle("""{"v":1,"id":"A3","type":"send","data":{"to":"+1"}}""") { replies += it }
        assertEquals("A3", lastNack().ref)
        assertEquals("invalid", lastNack().code)
    }
}
