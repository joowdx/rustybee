package dev.joowdx.rustybee

import dev.joowdx.rustybee.protocol.Envelope
import dev.joowdx.rustybee.protocol.FrameType
import dev.joowdx.rustybee.protocol.Frames
import dev.joowdx.rustybee.protocol.Health
import dev.joowdx.rustybee.protocol.PROTOCOL_VERSION
import dev.joowdx.rustybee.protocol.PairRequest
import dev.joowdx.rustybee.protocol.PairResponse
import dev.joowdx.rustybee.protocol.ProtocolJson
import dev.joowdx.rustybee.protocol.SetSim
import dev.joowdx.rustybee.protocol.SimInfo
import kotlinx.serialization.KSerializer
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** AC-002-01 / AC-011-02: every fixture parses into the Kotlin types and re-serialises equivalently. */
class ProtocolFixturesTest {

    /** Missing optional fields and explicit nulls are equivalent (spec 002). */
    private fun normalize(e: JsonElement): JsonElement = when (e) {
        is JsonObject -> JsonObject(e.filterValues { it !is JsonNull }.mapValues { normalize(it.value) })
        is JsonArray -> JsonArray(e.map(::normalize))
        else -> e
    }

    @Suppress("UNCHECKED_CAST")
    private fun roundTrip(serializer: KSerializer<*>, element: JsonElement): JsonElement {
        val s = serializer as KSerializer<Any?>
        val obj = ProtocolJson.decodeFromJsonElement(s, element)
        return ProtocolJson.parseToJsonElement(ProtocolJson.encodeToString(s, obj))
    }

    @Test
    fun ac_002_01_every_fixture_round_trips() {
        val files = Fixtures.dir.listFiles { f -> f.name.endsWith(".json") }!!.sortedBy { it.name }
        assertTrue("expected fixtures", files.size >= 13)
        for (file in files) {
            val original = ProtocolJson.parseToJsonElement(file.readText())
            val again = when (file.name) {
                "pair_request.json" -> roundTrip(PairRequest.serializer(), original)
                "pair_response.json" -> roundTrip(PairResponse.serializer(), original)
                else -> {
                    val env = ProtocolJson.decodeFromJsonElement(Envelope.serializer(), original)
                    assertEquals(file.name, PROTOCOL_VERSION, env.v)
                    val ser = Frames.dataSerializer(env.type)
                    assertNotNull(file.name, ser)
                    val data = roundTrip(ser!!, env.data)
                    ProtocolJson.parseToJsonElement(
                        ProtocolJson.encodeToString(Envelope.serializer(), env.copy(data = data)),
                    )
                }
            }
            assertEquals(file.name, normalize(original), normalize(again))
        }
    }

    @Test
    fun ac_002_20_set_sim_frame() {
        val env = ProtocolJson.decodeFromString(Envelope.serializer(), Fixtures.read("set_sim.json"))
        assertEquals(FrameType.SET_SIM, env.type)
        assertEquals(SetSim(1), ProtocolJson.decodeFromJsonElement(SetSim.serializer(), env.data))
        // "Phone default" is a null sim, omitted on output like every null optional.
        val frame = Frames.encode(FrameType.SET_SIM, SetSim.serializer(), SetSim(null), "01JA0000000000000000000014")
        assertEquals("{\"v\":1,\"id\":\"01JA0000000000000000000014\",\"type\":\"set_sim\",\"data\":{}}", frame)
        assertEquals(SetSim(null), ProtocolJson.decodeFromString(SetSim.serializer(), "{\"sim\":null}"))
    }

    @Test
    fun ac_002_21_health_sims_optional() {
        val env = ProtocolJson.decodeFromString(Envelope.serializer(), Fixtures.read("health.json"))
        val health = ProtocolJson.decodeFromJsonElement(Health.serializer(), env.data)
        assertEquals(listOf(SimInfo(1, 0, "Example Mobile", null)), health.sims)
        // Without READ_PHONE_STATE the list is left out, so the server keeps what it had.
        val text = ProtocolJson.encodeToString(Health.serializer(), health.copy(sims = null))
        assertTrue(!text.contains("\"sims\""))
        // An empty list is sent: it means no SIM.
        assertTrue(ProtocolJson.encodeToString(Health.serializer(), health.copy(sims = emptyList())).contains("\"sims\":[]"))
    }

    @Test
    fun ac_011_02_unknown_fields_are_ignored() {
        val text = Fixtures.read("send.json").replace("\"sim\":null", "\"sim\":null,\"future\":{\"x\":1}")
        val env = ProtocolJson.decodeFromString(Envelope.serializer(), text)
        val send = ProtocolJson.decodeFromJsonElement(dev.joowdx.rustybee.protocol.Send.serializer(), env.data)
        assertEquals("01JA0000000000000000MSG001", send.msgId)
    }
}
