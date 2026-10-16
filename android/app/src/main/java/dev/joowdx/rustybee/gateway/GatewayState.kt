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
    val filtersSave: FiltersSave = FiltersSave.Idle,
)

/** Progress of the last filter edit sent from the phone. */
sealed interface FiltersSave {
    data object Idle : FiltersSave
    data object Saving : FiltersSave
    data object Saved : FiltersSave
    /** The server's filters changed since the edit started; the new ones are in prefs. */
    data object Conflict : FiltersSave
    data class Failed(val message: String) : FiltersSave
}
