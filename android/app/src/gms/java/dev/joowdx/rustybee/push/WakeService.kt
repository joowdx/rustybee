package dev.joowdx.rustybee.push

import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import dev.joowdx.rustybee.gateway.GatewayService
import dev.joowdx.rustybee.gateway.Graph

/** Starts the gateway on a high-priority `{"t":"wake"}` data message. Never carries content. */
class WakeService : FirebaseMessagingService() {
    override fun onMessageReceived(message: RemoteMessage) {
        if (message.data["t"] != "wake") return
        val prefs = Graph.of(this).prefs
        if (prefs.paired && prefs.gatewayEnabled) GatewayService.start(this)
    }

    override fun onNewToken(token: String) {
        Graph.of(this).prefs.fcmToken = token
        // The next hello carries the new token.
    }
}
