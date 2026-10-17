package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.Cleartext
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** AC-011-12: cleartext only to LAN hosts. */
class CleartextTest {
    @Test
    fun ac_011_12_tls_is_always_allowed() {
        assertTrue(Cleartext.allowed("https://sms.example.com"))
        assertTrue(Cleartext.allowed("wss://sms.example.com/api/device/ws"))
    }

    @Test
    fun ac_011_12_cleartext_to_lan_hosts_is_allowed() {
        listOf(
            "http://127.0.0.1:8080", "ws://127.8.9.10/ws",
            "http://10.0.2.2:3000", "ws://10.255.255.255/api/device/ws",
            "http://172.16.0.1", "http://172.31.255.254",
            "http://192.168.1.20:8080",
            "http://169.254.10.10",
            "http://[::1]:8080", "ws://[fe80::1]/ws", "http://[fd12:3456::1]", "http://[fc00::5]",
            "http://[::ffff:192.168.1.2]",
            "http://localhost:8080", "http://LocalHost", "http://dev.localhost",
            "http://gateway.local", "ws://pi.local./ws",
        ).forEach { assertTrue(it, Cleartext.allowed(it)) }
    }

    @Test
    fun ac_011_12_cleartext_to_public_hosts_is_refused() {
        listOf(
            "http://sms.example.com", "ws://sms.example.com/api/device/ws",
            "http://8.8.8.8", "http://172.15.0.1", "http://172.32.0.1", "http://192.169.1.1",
            "http://169.255.0.1", "http://11.0.0.1", "http://100.64.0.1",
            "http://[2001:db8::1]", "http://[fe00::1]", "http://[::ffff:8.8.8.8]",
            "http://localhost.example.com", "http://local", "http://example.local.evil.com",
            "http://10.0.0", "http://10.0.0.256", "http://", "ws:///path",
        ).forEach { assertFalse(it, Cleartext.allowed(it)) }
    }

    @Test
    fun ac_011_12_names_are_never_resolved() {
        // A public name that resolves to a private IP is still refused: only the literal host counts.
        assertFalse(Cleartext.isLanHost("10.0.0.1.nip.io"))
    }
}
