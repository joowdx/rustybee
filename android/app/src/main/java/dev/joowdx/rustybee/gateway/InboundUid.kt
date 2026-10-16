package dev.joowdx.rustybee.gateway

import java.security.MessageDigest

/** Spec 002: lowercase hex of the first 16 bytes of SHA-256(from\nreceived_at\nbody\nsim). */
object InboundUid {
    fun of(from: String, receivedAt: Long, body: String, sim: Int?): String {
        val input = "$from\n$receivedAt\n$body\n${sim?.toString() ?: ""}"
        val digest = MessageDigest.getInstance("SHA-256").digest(input.toByteArray(Charsets.UTF_8))
        return digest.copyOf(16).joinToString("") { "%02x".format(it) }
    }
}
