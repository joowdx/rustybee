package dev.joowdx.rustybee.gateway

import android.Manifest
import android.annotation.SuppressLint
import android.app.ActivityManager
import android.app.ApplicationStartInfo
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.location.LocationManager
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.wifi.WifiInfo
import android.net.wifi.WifiManager
import android.os.BatteryManager
import android.os.Build
import android.os.PowerManager
import android.os.Process
import android.os.SystemClock
import android.provider.Settings
import android.telephony.SignalStrength
import android.telephony.SubscriptionManager
import android.telephony.TelephonyDisplayInfo
import android.telephony.TelephonyManager
import dev.joowdx.rustybee.BuildConfig
import dev.joowdx.rustybee.protocol.Battery
import dev.joowdx.rustybee.protocol.Health
import dev.joowdx.rustybee.protocol.Hello
import dev.joowdx.rustybee.protocol.Lifecycle
import dev.joowdx.rustybee.protocol.LifecycleExit
import dev.joowdx.rustybee.protocol.LifecycleStart
import dev.joowdx.rustybee.protocol.Permissions
import dev.joowdx.rustybee.protocol.Radios
import dev.joowdx.rustybee.protocol.SimInfo
import dev.joowdx.rustybee.push.Push
import java.util.concurrent.atomic.AtomicBoolean

/** Builds `hello` and `health` payloads from the framework. */
class Telemetry(private val context: Context) {

    fun granted(p: String) = context.checkSelfPermission(p) == PackageManager.PERMISSION_GRANTED

    fun permissions(): Permissions {
        val nm = context.getSystemService(NotificationManager::class.java)
        val channel = nm.getNotificationChannel(Notifications.CHANNEL_GATEWAY)
        // A channel that has not been created yet is not blocked.
        val channelOpen = channel == null || channel.importance != NotificationManager.IMPORTANCE_NONE
        return Permissions(
            sendSms = granted(Manifest.permission.SEND_SMS),
            receiveSms = granted(Manifest.permission.RECEIVE_SMS),
            notifications = granted(Manifest.permission.POST_NOTIFICATIONS) &&
                channelOpen &&
                nm.areNotificationsEnabled(),
            batteryUnrestricted = batteryUnrestricted(),
            readPhoneState = granted(Manifest.permission.READ_PHONE_STATE),
        )
    }

    fun batteryUnrestricted(): Boolean =
        context.getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(context.packageName)

    fun battery(): Battery {
        val bm = context.getSystemService(BatteryManager::class.java)
        // "Charging" means on external power: `isCharging` is false while plugged in but held at a charge limit or full.
        val sticky = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        val plugged = (sticky?.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) ?: 0) != 0
        return Battery(bm.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY).coerceIn(0, 100), plugged || bm.isCharging)
    }

    fun network(): String {
        val cm = context.getSystemService(ConnectivityManager::class.java)
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return "none"
        return networkLabel(caps::hasTransport)
    }

    @SuppressLint("MissingPermission")
    fun sims(): List<SimInfo> {
        if (!granted(Manifest.permission.READ_PHONE_STATE)) return emptyList()
        val sm = context.getSystemService(SubscriptionManager::class.java) ?: return emptyList()
        return sm.activeSubscriptionInfoList.orEmpty().map {
            SimInfo(it.subscriptionId, it.simSlotIndex, it.carrierName?.toString(), null)
        }
    }

    fun hello(outbox: Int, radios: Radios? = null): Hello = Hello(
        appVersion = BuildConfig.VERSION_NAME,
        appBuild = BuildConfig.VERSION_CODE,
        osVersion = Build.VERSION.RELEASE,
        manufacturer = Build.MANUFACTURER,
        model = Build.MODEL,
        sims = sims(),
        battery = battery(),
        network = network(),
        fcmToken = Push.token(context),
        permissions = permissions(),
        outbox = outbox,
        lifecycle = processLifecycle,
        radios = radios,
    )

    fun health(outbox: Int, fcmToken: String? = null, network: String = network(), radios: Radios? = null): Health =
        Health(battery(), network, permissions(), outbox, fcmToken, radios)

    /** The data SIM's TelephonyManager, or the default one without a data SIM. */
    fun dataTelephony(): TelephonyManager {
        val base = context.getSystemService(TelephonyManager::class.java)
        val sub = SubscriptionManager.getDefaultDataSubscriptionId()
        return if (sub == SubscriptionManager.INVALID_SUBSCRIPTION_ID) base else base.createForSubscriptionId(sub)
    }

    /**
     * The data SIM's carrier, signal and generation, or null when no SIM is ready. [signal] and
     * [display] are the listeners' latest; without [display] the generation needs READ_PHONE_STATE.
     * Like the status bar, the generation shows only while mobile data is on. With [withSignal] off,
     * the bars and generation (never reported then) are not read.
     */
    @SuppressLint("MissingPermission")
    fun cellularReading(signal: SignalStrength?, display: TelephonyDisplayInfo?, withSignal: Boolean = true): RadioReading? {
        val tm = dataTelephony()
        if (tm.simState != TelephonyManager.SIM_STATE_READY) return null
        val ss = if (withSignal) signal ?: tm.signalStrength else null
        return RadioReading(
            name = tm.networkOperatorName?.trim()?.takeIf { it.isNotEmpty() },
            level = ss?.level,
            dbm = ss?.cellSignalStrengths?.firstOrNull()?.dbm?.takeIf { it in -150..-1 },
            tech = when {
                !withSignal -> null
                !runCatching { tm.isDataEnabled }.getOrDefault(false) -> null
                display != null -> RadioReport.tech(display.networkType, display.overrideNetworkType)
                granted(Manifest.permission.READ_PHONE_STATE) -> runCatching { RadioReport.tech(tm.dataNetworkType) }.getOrNull()
                else -> null
            },
        )
    }

    /** A connected Wi-Fi network's name and signal, from capabilities that carry location info. */
    fun wifiReading(caps: NetworkCapabilities): RadioReading {
        val info = caps.transportInfo as? WifiInfo
        val rssi = (info?.rssi ?: caps.signalStrength).takeIf { it in -127..-1 }
        val wm = context.getSystemService(WifiManager::class.java)
        return RadioReading(
            name = RadioReport.ssid(info?.ssid),
            level = rssi?.let { RadioReport.bars(wm.calculateSignalLevel(it), wm.maxSignalLevel) },
            dbm = rssi,
        )
    }

    /** What is missing for Android to reveal the Wi-Fi name to the background service, or null. */
    fun wifiNameBlocker(): String? = when {
        !granted(Manifest.permission.ACCESS_FINE_LOCATION) -> "Needs precise location permission."
        !granted(Manifest.permission.ACCESS_BACKGROUND_LOCATION) -> "Needs location set to \"Allow all the time\"."
        !context.getSystemService(LocationManager::class.java).isLocationEnabled -> "Needs Location turned on."
        else -> null
    }

    companion object {
        /** The protocol's `network` value; a VPN carries its underlying transports. */
        fun networkLabel(hasTransport: (Int) -> Boolean): String = when {
            hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "wifi"
            hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cellular"
            hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "ethernet"
            else -> "other"
        }

        private val captured = AtomicBoolean(false)

        @Volatile
        private var processLifecycle: Lifecycle? = null

        /** Read once per process, when [GatewayService] is created, and reuse for every hello. */
        fun captureLifecycle(context: Context) {
            if (!captured.compareAndSet(false, true)) return
            processLifecycle = runCatching { readLifecycle(context) }.onFailure {
                Graph.of(context).log("lifecycle unavailable: ${it.javaClass.simpleName}", "E")
            }.getOrNull()
        }

        private fun readLifecycle(context: Context): Lifecycle {
            val am = context.getSystemService(ActivityManager::class.java)
            val start = am.getHistoricalProcessStartReasons(1).firstOrNull()
                ?.takeIf { it.pid == Process.myPid() }
                ?.let { info ->
                    val launchNs = info.startupTimestamps[ApplicationStartInfo.START_TIMESTAMP_LAUNCH]
                    LifecycleStart(
                        reason = LifecycleReason.startReason(info.reason),
                        at = LifecycleReason.launchEpochMs(launchNs, SystemClock.uptimeNanos(), System.currentTimeMillis())
                            ?: 0L,
                        wasForceStopped = runCatching { info.wasForceStopped() }.getOrDefault(false),
                    )
                }
            val exit = am.getHistoricalProcessExitReasons(context.packageName, 0, 1).firstOrNull()?.let { info ->
                LifecycleExit(
                    reason = LifecycleReason.exitReason(info.reason),
                    at = info.timestamp,
                    description = LifecycleReason.description(info.description),
                )
            }
            return Lifecycle(start = start, exit = exit)
        }
    }

    fun deviceName(): String =
        Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME) ?: Build.MODEL
}
