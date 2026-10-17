package dev.joowdx.rustybee.gateway

import android.telephony.TelephonyDisplayInfo
import android.telephony.TelephonyManager
import dev.joowdx.rustybee.protocol.Radio
import dev.joowdx.rustybee.protocol.Radios

/** The four Network switches (spec 015); all off by default. */
data class RadioSwitches(
    val cellularName: Boolean = false,
    val cellularSignal: Boolean = false,
    val wifiName: Boolean = false,
    val wifiSignal: Boolean = false,
) {
    val cellular: Boolean get() = cellularName || cellularSignal
    val wifi: Boolean get() = wifiName || wifiSignal
}

/** What the phone can read for one radio, before the switches are applied. */
data class RadioReading(val name: String? = null, val level: Int? = null, val dbm: Int? = null, val tech: String? = null)

sealed interface RadioDecision {
    data object Skip : RadioDecision
    data object Now : RadioDecision
    data class Later(val delayMs: Long) : RadioDecision
}

/** Pure rules for `radios` (spec 015): what to put in it and when a change is worth a `health`. */
object RadioReport {
    const val MIN_LEVEL_INTERVAL_MS = 60_000L
    /** How long a newly started listener gets to deliver its first readings before a report. */
    const val SETTLE_MS = 2_000L
    private const val UNKNOWN_SSID = "<unknown ssid>"

    fun build(switches: RadioSwitches, cellular: RadioReading?, wifi: RadioReading?): Radios = Radios(
        cellular = cellular?.let { pick(it, switches.cellularName, switches.cellularSignal) },
        wifi = wifi?.let { pick(it, switches.wifiName, switches.wifiSignal) },
    )

    private fun pick(r: RadioReading, name: Boolean, signal: Boolean): Radio? {
        val out = Radio(
            name = r.name.takeIf { name },
            level = r.level?.coerceIn(0, 4).takeIf { signal },
            dbm = r.dbm.takeIf { signal },
            tech = r.tech.takeIf { signal },
        )
        return out.takeIf { it != Radio() }
    }

    /** A Wi-Fi name as Android reports it: quoted, or a placeholder when location access is missing. */
    fun ssid(raw: String?): String? {
        val s = raw?.trim()?.removeSurrounding("\"") ?: return null
        return s.takeIf { it.isNotEmpty() && it != UNKNOWN_SSID }
    }

    /** Android's 0..[max] level scaled to 0–4 bars. */
    fun bars(level: Int, max: Int): Int = if (max <= 0) 0 else ((level * 4 + max / 2) / max).coerceIn(0, 4)

    /**
     * The cellular generation ("2G" to "5G") as the status bar shows it: [override] is
     * [TelephonyDisplayInfo.getOverrideNetworkType], which marks 5G on an LTE anchor (NSA).
     */
    fun tech(networkType: Int, override: Int = TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_NONE): String? = when {
        override == TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_NR_NSA ||
            override == TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_NR_ADVANCED -> "5G"
        else -> when (networkType) {
            TelephonyManager.NETWORK_TYPE_NR -> "5G"
            TelephonyManager.NETWORK_TYPE_LTE -> "4G"
            TelephonyManager.NETWORK_TYPE_UMTS, TelephonyManager.NETWORK_TYPE_HSDPA,
            TelephonyManager.NETWORK_TYPE_HSUPA, TelephonyManager.NETWORK_TYPE_HSPA,
            TelephonyManager.NETWORK_TYPE_HSPAP, TelephonyManager.NETWORK_TYPE_TD_SCDMA,
            TelephonyManager.NETWORK_TYPE_EVDO_0, TelephonyManager.NETWORK_TYPE_EVDO_A,
            TelephonyManager.NETWORK_TYPE_EVDO_B, TelephonyManager.NETWORK_TYPE_EHRPD,
            -> "3G"
            TelephonyManager.NETWORK_TYPE_GPRS, TelephonyManager.NETWORK_TYPE_EDGE,
            TelephonyManager.NETWORK_TYPE_GSM, TelephonyManager.NETWORK_TYPE_CDMA,
            TelephonyManager.NETWORK_TYPE_1xRTT, TelephonyManager.NETWORK_TYPE_IDEN,
            -> "2G"
            else -> null
        }
    }

    /**
     * Whether [current] needs a `health`, given [reported] (the last sent) and [sinceLastMs] (time since
     * the last `hello` or `health`). Switch, name and mobile data changes go at once; bar and
     * generation changes at most once a minute; `dbm` alone never. While a listener that just started
     * is [settlingMs] from delivering its first readings, any change waits for them, so a half-read
     * state (no Wi-Fi yet, or 4G before the 5G display info) is never sent.
     */
    fun decide(welcomed: Boolean, reported: Radios?, current: Radios, sinceLastMs: Long?, settlingMs: Long = 0): RadioDecision {
        if (!welcomed || reported == current) return RadioDecision.Skip
        if (settlingMs > 0) return RadioDecision.Later(settlingMs)
        val noDbm = { r: Radios -> Radios(r.cellular?.copy(dbm = null), r.wifi?.copy(dbm = null)) }
        val noSignal = { r: Radios ->
            Radios(r.cellular?.copy(level = null, dbm = null, tech = null), r.wifi?.copy(level = null, dbm = null, tech = null))
        }
        if (reported != null && noDbm(reported) == noDbm(current)) return RadioDecision.Skip
        if (reported == null || noSignal(reported) != noSignal(current) || hasLevel(reported) != hasLevel(current)) {
            return RadioDecision.Now
        }
        val wait = MIN_LEVEL_INTERVAL_MS - (sinceLastMs ?: MIN_LEVEL_INTERVAL_MS)
        return if (wait <= 0) RadioDecision.Now else RadioDecision.Later(wait)
    }

    /** Which signal fields are present; one appearing or going (a switch, or mobile data on/off) reports at once. */
    private fun hasLevel(r: Radios) = Triple(r.cellular?.level != null, r.wifi?.level != null, r.cellular?.tech != null)
}
