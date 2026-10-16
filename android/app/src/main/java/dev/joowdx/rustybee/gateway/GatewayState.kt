package dev.joowdx.rustybee.gateway

enum class ConnectionState { STOPPED, CONNECTING, CONNECTED }

data class GatewayState(
    val connection: ConnectionState = ConnectionState.STOPPED,
    val lastError: String? = null,
    val outbox: Int = 0,
    val sendsToday: Int = 0,
    val receivesToday: Int = 0,
    /** The `filters_rev` of the config the phone holds. */
    val filtersRev: Long = 0,
    val forwardingSave: ForwardingSave = ForwardingSave.Idle,
)

/** Progress of the last forwarding edit sent from the phone. */
sealed interface ForwardingSave {
    data object Idle : ForwardingSave
    data object Saving : ForwardingSave
    data object Saved : ForwardingSave
    /** Forwarding was turned off offline: it applies now and reaches the server on reconnect. */
    data object SavedOnPhone : ForwardingSave
    /** The server's settings changed since the edit started; the new ones are in prefs. */
    data object Conflict : ForwardingSave
    data class Failed(val message: String) : ForwardingSave
}
