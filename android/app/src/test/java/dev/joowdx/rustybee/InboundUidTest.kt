package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.InboundUid
import dev.joowdx.rustybee.protocol.Envelope
import dev.joowdx.rustybee.protocol.Inbound
import dev.joowdx.rustybee.protocol.ProtocolJson
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Test

/** AC-012-03. */
class InboundUidTest {
    @Test
    fun ac_012_03_uid_matches_inbound_fixture() {
        val env = ProtocolJson.decodeFromString(Envelope.serializer(), Fixtures.read("inbound.json"))
        val inbound = ProtocolJson.decodeFromJsonElement(Inbound.serializer(), env.data)
        assertEquals(inbound.uid, InboundUid.of(inbound.from, inbound.receivedAt, inbound.body, inbound.sim))
    }

    @Test
    fun ac_012_03_null_sim_hashes_as_empty() {
        val a = InboundUid.of("+1", 1, "x", null)
        assertEquals(32, a.length)
        assertNotEquals(a, InboundUid.of("+1", 1, "x", 0))
    }
}
