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
    const val CHANNEL_GATEWAY = "gateway"
    const val CHANNEL_ALERTS = "alerts"
    const val ID_GATEWAY = 1
    const val ID_STOPPED = 2

    fun ensureChannels(context: Context) {
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_GATEWAY, context.getString(R.string.channel_gateway), NotificationManager.IMPORTANCE_LOW),
        )
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_ALERTS, context.getString(R.string.channel_alerts), NotificationManager.IMPORTANCE_HIGH),
        )
    }

    private fun openMain(context: Context): PendingIntent = PendingIntent.getActivity(
        context, 0,
        Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    fun gateway(context: Context, state: GatewayState): Notification {
        val title = context.getString(
            if (state.connection == ConnectionState.CONNECTED) R.string.state_connected else R.string.state_reconnecting,
        )
        return Notification.Builder(context, CHANNEL_GATEWAY)
            .setSmallIcon(R.drawable.ic_stat_bolt)
            .setContentTitle(title)
            .setContentText(context.getString(R.string.sends_today, state.sendsToday))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setCategory(Notification.CATEGORY_SERVICE)
            .setContentIntent(openMain(context))
            .build()
    }

    fun postStopped(context: Context) {
        ensureChannels(context)
        val n = Notification.Builder(context, CHANNEL_ALERTS)
            .setSmallIcon(R.drawable.ic_stat_bolt)
            .setContentTitle(context.getString(R.string.gateway_stopped))
            .setContentText(context.getString(R.string.tap_to_restart))
            .setAutoCancel(true)
            .setContentIntent(openMain(context))
            .build()
        runCatching { context.getSystemService(NotificationManager::class.java).notify(ID_STOPPED, n) }
    }
}
