package dev.joowdx.rustybee.gateway

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import dev.joowdx.rustybee.R
import dev.joowdx.rustybee.ui.MainActivity

object Notifications {
    // Channel settings are fixed once created, so turning badges off needed new ids.
    const val CHANNEL_GATEWAY = "gateway_v2"
    const val CHANNEL_ALERTS = "alerts_v2"
    private val OLD_CHANNELS = listOf("gateway", "alerts")
    const val ID_GATEWAY = 1
    const val ID_STOPPED = 2
    const val ID_UNPAIRED = 3

    fun ensureChannels(context: Context) {
        val nm = context.getSystemService(NotificationManager::class.java)
        OLD_CHANNELS.forEach(nm::deleteNotificationChannel)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_GATEWAY, context.getString(R.string.channel_gateway), NotificationManager.IMPORTANCE_LOW)
                .apply { setShowBadge(false) },
        )
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_ALERTS, context.getString(R.string.channel_alerts), NotificationManager.IMPORTANCE_HIGH)
                .apply { setShowBadge(false) },
        )
    }

    private fun openMain(context: Context): PendingIntent = PendingIntent.getActivity(
        context, 0,
        Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    /** What the gateway notification shows; it is re-posted only when this changes. */
    data class GatewayContent(val connected: Boolean, val sendsToday: Int)

    fun content(state: GatewayState) = GatewayContent(state.connection == ConnectionState.CONNECTED, state.sendsToday)

    fun gateway(context: Context, state: GatewayState): Notification {
        val title = context.getString(
            if (state.connection == ConnectionState.CONNECTED) R.string.state_connected else R.string.state_reconnecting,
        )
        return Notification.Builder(context, CHANNEL_GATEWAY)
            .setSmallIcon(R.drawable.ic_stat_bee)
            .setContentTitle(title)
            .setContentText(context.getString(R.string.sends_today, state.sendsToday))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setCategory(Notification.CATEGORY_SERVICE)
            .setContentIntent(openMain(context))
            .build()
    }

    /** The server dropped this device (spec 002); tapping opens Setup, which repeats [message]. */
    fun postUnpaired(context: Context, message: String) {
        ensureChannels(context)
        val n = Notification.Builder(context, CHANNEL_ALERTS)
            .setSmallIcon(R.drawable.ic_stat_bee)
            .setContentTitle(context.getString(R.string.gateway_unpaired))
            .setContentText(message)
            .setStyle(Notification.BigTextStyle().bigText(message))
            .setAutoCancel(true)
            .setContentIntent(openMain(context))
            .build()
        runCatching { context.getSystemService(NotificationManager::class.java).notify(ID_UNPAIRED, n) }
    }

    fun postStopped(context: Context) {
        ensureChannels(context)
        val n = Notification.Builder(context, CHANNEL_ALERTS)
            .setSmallIcon(R.drawable.ic_stat_bee)
            .setContentTitle(context.getString(R.string.gateway_stopped))
            .setContentText(context.getString(R.string.tap_to_restart))
            .setAutoCancel(true)
            .setContentIntent(openMain(context))
            .build()
        runCatching { context.getSystemService(NotificationManager::class.java).notify(ID_STOPPED, n) }
    }
}
