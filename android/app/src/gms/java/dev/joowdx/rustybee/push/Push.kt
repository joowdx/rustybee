package dev.joowdx.rustybee.push

import android.content.Context
import com.google.firebase.messaging.FirebaseMessaging
import dev.joowdx.rustybee.gateway.Graph
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull
import kotlin.coroutines.resume

/** gms flavor: FCM token used only to wake the app (spec 009). */
object Push {
    const val SUPPORTED = true

    private const val TOKEN_TIMEOUT_MS = 5_000L

    fun token(context: Context): String? = Graph.of(context).prefs.fcmToken

    /** Waits up to 5 s for the current token, then returns that or any token already stored. */
    suspend fun refresh(context: Context): String? {
        val fresh = withTimeoutOrNull(TOKEN_TIMEOUT_MS) { awaitToken(context) }
        return fresh ?: Graph.of(context).prefs.fcmToken
    }

    private suspend fun awaitToken(context: Context): String? = suspendCancellableCoroutine { cont ->
        val task = runCatching { FirebaseMessaging.getInstance().token }.getOrElse {
            cont.resume(null)
            return@suspendCancellableCoroutine
        }
        task.addOnCompleteListener { done ->
            val token = done.takeIf { it.isSuccessful }?.result
            if (!token.isNullOrEmpty()) Graph.of(context).prefs.fcmToken = token
            if (cont.isActive) cont.resume(token)
        }
    }
}
