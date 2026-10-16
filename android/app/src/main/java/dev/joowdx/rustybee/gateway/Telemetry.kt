package dev.joowdx.rustybee.gateway

import android.Manifest
import android.annotation.SuppressLint
import android.app.NotificationManager
import android.content.Context
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.BatteryManager
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import android.telephony.SubscriptionManager
import dev.joowdx.rustybee.BuildConfig
import dev.joowdx.rustybee.protocol.Battery
import dev.joowdx.rustybee.protocol.Health
import dev.joowdx.rustybee.protocol.Hello
import dev.joowdx.rustybee.protocol.Permissions
import dev.joowdx.rustybee.protocol.SimInfo
import dev.joowdx.rustybee.push.Push

/** Builds `hello` and `health` payloads from the framework. */
class Telemetry(private val context: Context) {

    private fun granted(p: String) = context.checkSelfPermission(p) == PackageManager.PERMISSION_GRANTED

    fun permissions(): Permissions = Permissions(
        sendSms = granted(Manifest.permission.SEND_SMS),
        receiveSms = granted(Manifest.permission.RECEIVE_SMS),
        notifications = granted(Manifest.permission.POST_NOTIFICATIONS) &&
            context.getSystemService(NotificationManager::class.java).areNotificationsEnabled(),
        batteryUnrestricted = batteryUnrestricted(),
    )

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
    )

    fun health(outbox: Int): Health = Health(battery(), network(), permissions(), outbox)

    fun deviceName(): String =
        Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME) ?: Build.MODEL
}
