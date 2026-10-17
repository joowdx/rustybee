package dev.joowdx.rustybee.gateway

import java.net.Inet6Address
import java.net.InetAddress
import java.net.URI

/**
 * Spec 011 AC-011-12: plain `http://` and `ws://` are allowed only to LAN hosts. The network
 * security config can't express IP ranges, so this check runs in the app, at pairing and at connect.
 */
object Cleartext {
    const val REFUSED = "Plain http/ws is only allowed to a LAN address (a private, link-local or loopback IP, " +
        "localhost or a .local name). Use https for this server."

    fun isCleartext(url: String): Boolean {
        val u = url.trim().lowercase()
        return u.startsWith("http://") || u.startsWith("ws://")
    }

    /** True for https/wss, and for http/ws to a LAN host. A URL without a host is refused. */
    fun allowed(url: String): Boolean {
        if (!isCleartext(url)) return true
        val host = runCatching { URI(url.trim()).host }.getOrNull() ?: return false
        return isLanHost(host)
    }

    fun isLanHost(rawHost: String): Boolean {
        val host = rawHost.trim().trimEnd('.').removePrefix("[").removeSuffix("]").lowercase()
        if (host.isEmpty()) return false
        if (host == "localhost" || host.endsWith(".localhost") || host.endsWith(".local")) return true
        ipv4(host)?.let { return isLanV4(it) }
        if (':' in host && host.all { it.isLetterOrDigit() || it == ':' || it == '.' || it == '%' }) {
            // An IPv6 literal: InetAddress parses it without a DNS lookup.
            val addr = runCatching { InetAddress.getByName(host) }.getOrNull() ?: return false
            val b = addr.address
            if (addr !is Inet6Address) return b.size == 4 && isLanV4(b.map { it.toInt() and 0xFF }) // IPv4-mapped
            return addr.isLoopbackAddress || addr.isLinkLocalAddress || (b[0].toInt() and 0xFE) == 0xFC
        }
        return false
    }

    private fun ipv4(host: String): List<Int>? {
        val parts = host.split('.')
        if (parts.size != 4) return null
        return parts.map { p ->
            if (p.isEmpty() || p.length > 3 || !p.all(Char::isDigit)) return null
            p.toInt().takeIf { it <= 255 } ?: return null
        }
    }

    private fun isLanV4(o: List<Int>): Boolean = when {
        o[0] == 127 -> true // loopback
        o[0] == 10 -> true
        o[0] == 172 && o[1] in 16..31 -> true
        o[0] == 192 && o[1] == 168 -> true
        o[0] == 169 && o[1] == 254 -> true // link-local
        else -> false
    }
}
