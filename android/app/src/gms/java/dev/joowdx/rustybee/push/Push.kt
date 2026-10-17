package dev.joowdx.rustybee.push

import android.content.Context
import com.google.firebase.messaging.FirebaseMessaging
import dev.joowdx.rustybee.gateway.Graph
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withTimeoutOrNull

/**
 * gms flavor: FCM token used only to wake the app (spec 009). Uses the `register()` flow, enabled by
 * the `firebase_messaging_installation_id_enabled` meta-data: every `register()` ends in
 * [WakeService.onRegistered], which delivers the token. The Task itself carries none.
 */
object Push {
    const val SUPPORTED = true

    private const val TOKEN_TIMEOUT_MS = 5_000L

    @Volatile
    private var waiting: CompletableDeferred<String>? = null

    fun token(context: Context): String? = Graph.of(context).prefs.fcmToken

    /** Waits up to 5 s for the current token, then returns that or any token already stored. */
    suspend fun refresh(context: Context): String? {
        val fresh = withTimeoutOrNull(TOKEN_TIMEOUT_MS) { awaitToken() }
        return fresh ?: Graph.of(context).prefs.fcmToken
    }

    /** Called by [WakeService.onRegistered] once the token is stored. */
    internal fun delivered(token: String) {
        waiting?.complete(token)
    }

    private suspend fun awaitToken(): String? {
        val deferred = waiting?.takeIf { it.isActive } ?: CompletableDeferred<String>().also { waiting = it }
        val started = runCatching { FirebaseMessaging.getInstance().register() }.getOrNull() ?: return null
        started.addOnFailureListener { deferred.cancel() }
        return runCatching { deferred.await() }.getOrNull()
    }
}
