package dev.joowdx.rustybee.gateway

enum class ConnectionState { STOPPED, CONNECTING, CONNECTED }

data class GatewayState(
    val connection: ConnectionState = ConnectionState.STOPPED,
    val lastError: String? = null,
    val outbox: Int = 0,
    val sendsToday: Int = 0,
    val receivesToday: Int = 0,
)
