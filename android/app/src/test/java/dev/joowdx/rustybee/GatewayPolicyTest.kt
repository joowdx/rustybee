package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.CLOSE_GATEWAY_OFF
import dev.joowdx.rustybee.gateway.CloseDecision
import dev.joowdx.rustybee.gateway.ConnectDecision
import dev.joowdx.rustybee.gateway.GatewayPolicy
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** The service's close and connect rules: AC-002-07, AC-002-17, AC-002-18 and AC-013-04. */
class GatewayPolicyTest {
    private fun closed(code: Int, http: Int? = null, stopping: Boolean = false, ownOff: Boolean = false) =
        GatewayPolicy.onClosed(code, http, stopping, ownOff)

    private fun connect(stopping: Boolean = false, suppressed: Boolean = false, paired: Boolean = true, token: Boolean = true) =
        GatewayPolicy.onConnect(stopping, suppressed, paired, token)

    @Test
    fun ac_002_07_revoked_close_unpairs() {
        assertEquals(CloseDecision.Unpair(GatewayPolicy.NOTICE_REVOKED), closed(4003))
        // Even while stopping, a revoke still wipes the credentials.
        assertTrue(closed(4003, stopping = true) is CloseDecision.Unpair)
    }

    @Test
    fun ac_002_17_no_reconnect_after_own_gateway_off() {
        assertEquals(CloseDecision.Stay, closed(CLOSE_GATEWAY_OFF, ownOff = true))
        assertEquals(ConnectDecision.Skip, connect(suppressed = true))
    }

    @Test
    fun ac_002_17_server_close_with_4010_still_reconnects() {
        assertEquals(CloseDecision.Reconnect, closed(CLOSE_GATEWAY_OFF, ownOff = false))
    }

    @Test
    fun ac_002_18_http_401_at_upgrade_unpairs_instead_of_retrying() {
        assertEquals(CloseDecision.Unpair(GatewayPolicy.NOTICE_UNAUTHORIZED), closed(-1, http = 401))
    }

    @Test
    fun ac_002_18_other_upgrade_failures_reconnect() {
        assertEquals(CloseDecision.Reconnect, closed(-1, http = 502))
        assertEquals(CloseDecision.Reconnect, closed(-1, http = 403))
        assertEquals(CloseDecision.Reconnect, closed(-1))
        assertEquals(CloseDecision.Reconnect, closed(1006))
        assertEquals(CloseDecision.Reconnect, closed(4000))
    }

    @Test
    fun ac_013_04_unpair_stops_the_service() {
        // Unpair stops the service first: a late close does not reconnect, a late connect does nothing.
        assertEquals(CloseDecision.Stay, closed(1000, stopping = true))
        assertEquals(ConnectDecision.Skip, connect(stopping = true, paired = false, token = false))
        // A connect that finds the pairing gone stops the service.
        assertEquals(ConnectDecision.Shutdown, connect(paired = false, token = false))
    }

    @Test
    fun connect_opens_when_paired_and_retries_an_unreadable_token() {
        assertEquals(ConnectDecision.Open, connect())
        assertEquals(ConnectDecision.Retry, connect(token = false))
    }
}
