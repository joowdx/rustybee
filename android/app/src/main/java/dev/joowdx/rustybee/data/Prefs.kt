package dev.joowdx.rustybee.data

import android.content.Context
import android.content.SharedPreferences
import dev.joowdx.rustybee.protocol.Config
import dev.joowdx.rustybee.protocol.ProtocolJson

/** Plain SharedPreferences; only the device token is encrypted (spec 011). */
class Prefs(context: Context) {
    private val sp: SharedPreferences = context.getSharedPreferences("rustybee", Context.MODE_PRIVATE)

    var serverUrl: String?
        get() = sp.getString("server_url", null)
        set(v) = sp.edit().putString("server_url", v).apply()

    var wsUrl: String?
        get() = sp.getString("ws_url", null)
        set(v) = sp.edit().putString("ws_url", v).apply()

    var deviceId: String?
        get() = sp.getString("device_id", null)
        set(v) = sp.edit().putString("device_id", v).apply()

    var allowInsecure: Boolean
        get() = sp.getBoolean("allow_insecure", false)
        set(v) = sp.edit().putBoolean("allow_insecure", v).apply()

    /** The last decrypted token, keyed by its ciphertext, so [paired] checks don't hit the Keystore each time. */
    @Volatile
    private var tokenCache: Pair<String, String>? = null

    var token: String?
        get() = readToken()
        set(v) {
            tokenCache = null
            sp.edit().putString("token_enc", v?.let(Crypto::encrypt)).apply()
        }

    private fun readToken(): String? {
        val enc = sp.getString("token_enc", null) ?: return null
        tokenCache?.let { (e, plain) -> if (e == enc) return plain }
        return when (val r = Crypto.decryptResult(enc)) {
            is Crypto.Decrypted.Ok -> r.plain.also { tokenCache = enc to it }
            Crypto.Decrypted.KeyLost -> {
                // The Keystore key is gone, so this pairing can never authenticate again (spec 011).
                clearPairing(NOTICE_KEY_LOST)
                null
            }
            Crypto.Decrypted.Failed -> null
        }
    }

    /**
     * Paired means a server URL and a token that decrypts. A token whose Keystore key is lost clears the
     * pairing, and Setup explains why. A transient Keystore failure still counts as paired.
     */
    val paired: Boolean
        get() {
            if (wsUrl == null || !sp.contains("token_enc")) return false
            readToken()
            return sp.contains("token_enc")
        }

    var gatewayEnabled: Boolean
        get() = sp.getBoolean("gateway_enabled", false)
        set(v) = sp.edit().putBoolean("gateway_enabled", v).apply()

    /** The user turned forwarding off on this phone; no server config turns it back on (spec 002). */
    var forwardingOffHere: Boolean
        get() = sp.getBoolean("forwarding_off_here", false)
        set(v) = sp.edit().putBoolean("forwarding_off_here", v).apply()

    var fcmToken: String?
        get() = sp.getString("fcm_token", null)
        set(v) = sp.edit().putString("fcm_token", v).apply()

    var lastConnectedAt: Long
        get() = sp.getLong("last_connected_at", 0)
        set(v) = sp.edit().putLong("last_connected_at", v).apply()

    var config: Config
        get() = sp.getString("config", null)
            ?.let { runCatching { ProtocolJson.decodeFromString(Config.serializer(), it) }.getOrNull() }
            ?: Config()
        set(v) = sp.edit().putString("config", ProtocolJson.encodeToString(Config.serializer(), v)).apply()

    /** Per-day counters ("sends", "receives"), reset when the local date changes. */
    fun today(counter: String): Int =
        if (sp.getLong("${counter}_day", -1) == epochDay()) sp.getInt("${counter}_count", 0) else 0

    @Synchronized
    fun bump(counter: String) {
        val n = today(counter) + 1
        sp.edit().putLong("${counter}_day", epochDay()).putInt("${counter}_count", n).apply()
    }

    private fun epochDay(): Long = java.time.LocalDate.now().toEpochDay()

    /** Monotonic PendingIntent request codes (spec 012 §4). */
    @Synchronized
    fun nextRequestCode(): Int {
        val next = sp.getInt("request_code", 0) + 1
        sp.edit().putInt("request_code", if (next == Int.MAX_VALUE) 1 else next).commit()
        return next
    }

    /** Why the app was unpaired without the user asking; Setup shows it until the next pairing. */
    val setupNotice: String?
        get() = sp.getString("setup_notice", null)

    /**
     * Wipes credentials and settings; keeps the FCM token, which belongs to the install.
     * [notice] tells Setup why, when the pairing was dropped without the user asking.
     */
    fun clearPairing(notice: String? = null) {
        tokenCache = null
        val fcm = fcmToken
        sp.edit().clear().putString("fcm_token", fcm).putString("setup_notice", notice).commit()
        Crypto.deleteKey()
    }

    companion object {
        const val NOTICE_KEY_LOST = "This phone's saved server token can no longer be decrypted: its Android Keystore key " +
            "is gone, for example after a backup restore or a security reset. Create a new pairing code in the web UI " +
            "and pair again."
    }
}
