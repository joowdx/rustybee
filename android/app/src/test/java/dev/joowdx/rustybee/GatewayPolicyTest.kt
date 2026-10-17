package dev.joowdx.rustybee

import android.net.NetworkCapabilities
import dev.joowdx.rustybee.gateway.CLOSE_GATEWAY_OFF
import dev.joowdx.rustybee.gateway.CloseDecision
import dev.joowdx.rustybee.gateway.ConnectDecision
import dev.joowdx.rustybee.gateway.GatewayPolicy
import dev.joowdx.rustybee.gateway.Telemetry
import org.junit.Assert.assertEquals
import org.junit.Test

/** The service's close and connect rules: AC-002-07, AC-002-17, AC-002-18, AC-002-19 and AC-013-04. */
class GatewayPolicyTest {
    private fun closed(code: Int, http: Int? = null, stopping: Boolean = false, ownOff: Boolean = false) =
        GatewayPolicy.onClosed(code, http, stopping, ownOff)

    private fun connect(stopping: Boolean = false, suppressed: Boolean = false, paired: Boolean = true, token: Boolean = true) =
        GatewayPolicy.onConnect(stopping, suppressed, paired, token)

    @Test
    fun ac_002_07_revoked_close_unpairs() {
        assertEquals(CloseDecision.Unpair(GatewayPolicy.NOTICE_REVOKED), closed(4003))
    }

    @Test
    fun ac_013_08_close_from_a_stopping_instance_never_unpairs() {
        // A re-pair stops the old instance; its late 4003 or 401 must not wipe the new pairing.
        assertEquals(CloseDecision.Stay, closed(4003, stopping = true))
        assertEquals(CloseDecision.Stay, closed(-1, http = 401, stopping = true))
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

    @Test
    fun ac_002_19_network_type_change_sends_health() {
        assertEquals(true, GatewayPolicy.reportNetwork(welcomed = true, reported = "cellular", current = "wifi"))
        assertEquals(true, GatewayPolicy.reportNetwork(welcomed = true, reported = "wifi", current = "cellular"))
        assertEquals(false, GatewayPolicy.reportNetwork(welcomed = true, reported = "wifi", current = "wifi"))
        // Before welcome a health must not precede the hello; with no callback yet there is nothing to correct.
        assertEquals(false, GatewayPolicy.reportNetwork(welcomed = false, reported = "cellular", current = "wifi"))
        assertEquals(false, GatewayPolicy.reportNetwork(welcomed = true, reported = "cellular", current = null))
    }

    @Test
    fun ac_002_19_network_label_maps_transports() {
        fun label(vararg t: Int) = Telemetry.networkLabel { it in t }
        assertEquals("wifi", label(NetworkCapabilities.TRANSPORT_WIFI, NetworkCapabilities.TRANSPORT_VPN))
        assertEquals("cellular", label(NetworkCapabilities.TRANSPORT_CELLULAR, NetworkCapabilities.TRANSPORT_VPN))
        assertEquals("ethernet", label(NetworkCapabilities.TRANSPORT_ETHERNET))
        assertEquals("other", label(NetworkCapabilities.TRANSPORT_VPN))
    }
}
