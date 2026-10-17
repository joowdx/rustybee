package dev.joowdx.rustybee.gateway

import android.Manifest
import android.annotation.SuppressLint
import android.app.ActivityManager
import android.app.ApplicationStartInfo
import android.app.NotificationManager
import android.content.Context
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.BatteryManager
import android.os.Build
import android.os.PowerManager
import android.os.Process
import android.os.SystemClock
import android.provider.Settings
import android.telephony.SubscriptionManager
import dev.joowdx.rustybee.BuildConfig
import dev.joowdx.rustybee.protocol.Battery
import dev.joowdx.rustybee.protocol.Health
import dev.joowdx.rustybee.protocol.Hello
import dev.joowdx.rustybee.protocol.Lifecycle
import dev.joowdx.rustybee.protocol.LifecycleExit
import dev.joowdx.rustybee.protocol.LifecycleStart
import dev.joowdx.rustybee.protocol.Permissions
import dev.joowdx.rustybee.protocol.SimInfo
import dev.joowdx.rustybee.push.Push
import java.util.concurrent.atomic.AtomicBoolean

/** Builds `hello` and `health` payloads from the framework. */
class Telemetry(private val context: Context) {

    private fun granted(p: String) = context.checkSelfPermission(p) == PackageManager.PERMISSION_GRANTED

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
        return Battery(bm.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY).coerceIn(0, 100), bm.isCharging)
    }

    fun network(): String {
        val cm = context.getSystemService(ConnectivityManager::class.java)
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return "none"
        return when {
            caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "wifi"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cellular"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "ethernet"
            else -> "other"
        }
    }

    @SuppressLint("MissingPermission")
    fun sims(): List<SimInfo> {
        if (!granted(Manifest.permission.READ_PHONE_STATE)) return emptyList()
        val sm = context.getSystemService(SubscriptionManager::class.java) ?: return emptyList()
        return sm.activeSubscriptionInfoList.orEmpty().map {
            SimInfo(it.subscriptionId, it.simSlotIndex, it.carrierName?.toString(), null)
        }
    }

    fun hello(outbox: Int): Hello = Hello(
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
    )

    fun health(outbox: Int, fcmToken: String? = null): Health =
        Health(battery(), network(), permissions(), outbox, fcmToken)

    companion object {
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
