package dev.joowdx.rustybee.push

import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import dev.joowdx.rustybee.gateway.GatewayService
import dev.joowdx.rustybee.gateway.Graph
import dev.joowdx.rustybee.gateway.Notifications

/** Starts the gateway on a high-priority `{"t":"wake"}` data message. Never carries content. */
class WakeService : FirebaseMessagingService() {
    override fun onMessageReceived(message: RemoteMessage) {
        if (message.data["t"] != "wake") return
        val prefs = Graph.of(this).prefs
        if (!prefs.paired || !prefs.gatewayEnabled) {
            Graph.of(this).log("wake ignored: not paired or gateway off")
            return
        }
        if (!GatewayService.start(this)) Notifications.postStopped(this)
    }

    /** A new or refreshed token, after every `register()` and on each FCM rotation (spec 009). */
    override fun onRegistered(token: String) {
        if (token.isEmpty()) return
        Graph.of(this).prefs.fcmToken = token
        GatewayService.onFcmToken(token)
        Push.delivered(token)
    }
}
