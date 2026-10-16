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

    var token: String?
        get() = sp.getString("token_enc", null)?.let(Crypto::decrypt)
        set(v) = sp.edit().putString("token_enc", v?.let(Crypto::encrypt)).apply()

    val paired: Boolean
        get() = sp.contains("token_enc") && wsUrl != null

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

    /** Wipes credentials and settings; keeps the FCM token, which belongs to the install. */
    fun clearPairing() {
        val fcm = fcmToken
        sp.edit().clear().putString("fcm_token", fcm).commit()
        Crypto.deleteKey()
    }
}
