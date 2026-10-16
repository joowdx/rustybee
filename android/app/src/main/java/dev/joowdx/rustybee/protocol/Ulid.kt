package dev.joowdx.rustybee.protocol

import java.security.SecureRandom

/** 26-character Crockford base32 ULIDs (48-bit ms timestamp + 80 random bits). */
object Ulid {
    private const val ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
    private val random = SecureRandom()

    fun next(now: Long = System.currentTimeMillis()): String {
        val rnd = ByteArray(10).also { random.nextBytes(it) }
        val out = CharArray(26)
        var t = now
        for (i in 9 downTo 0) {
            out[i] = ALPHABET[(t and 31).toInt()]
            t = t ushr 5
        }
        // 80 random bits -> 16 chars
        var bitBuf = 0L
        var bits = 0
        var idx = 10
        for (b in rnd) {
            bitBuf = (bitBuf shl 8) or (b.toLong() and 0xFF)
            bits += 8
            while (bits >= 5) {
                bits -= 5
                out[idx++] = ALPHABET[((bitBuf ushr bits) and 31).toInt()]
            }
        }
        return String(out)
    }
}
