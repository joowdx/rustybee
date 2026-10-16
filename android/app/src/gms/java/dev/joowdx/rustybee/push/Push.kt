package dev.joowdx.rustybee.push

import android.content.Context
import com.google.firebase.messaging.FirebaseMessaging
import dev.joowdx.rustybee.gateway.Graph

/** gms flavor: FCM token used only to wake the app (spec 009). */
object Push {
    const val SUPPORTED = true

    fun token(context: Context): String? = Graph.of(context).prefs.fcmToken

    fun refresh(context: Context) {
        runCatching {
            FirebaseMessaging.getInstance().token.addOnSuccessListener { Graph.of(context).prefs.fcmToken = it }
        }
    }
}
