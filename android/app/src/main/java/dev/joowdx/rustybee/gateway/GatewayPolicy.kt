package dev.joowdx.rustybee.gateway

/** What [GatewayService] does after its socket closes (spec 002). */
sealed interface CloseDecision {
    /** The server no longer accepts this device: wipe credentials, stop, alert and show Setup. */
    data class Unpair(val notice: String) : CloseDecision

    /** This side ended the connection (service stopping, unpair, or its own 4010): stay stopped. */
    data object Stay : CloseDecision

    data object Reconnect : CloseDecision
}

/** Whether [GatewayService] opens a socket when asked to connect. */
sealed interface ConnectDecision {
    data object Open : ConnectDecision

    /** Stopping, or the user switched the gateway off: do nothing. */
    data object Skip : ConnectDecision

    /** No pairing left (for example after an unpair): stop the service. */
    data object Shutdown : ConnectDecision

    /** Still paired but the token could not be read this time: try again after the backoff. */
    data object Retry : ConnectDecision
}

/** The service's close and connect rules, kept pure so they are JVM-testable like [Backoff]. */
object GatewayPolicy {
    const val CLOSE_REVOKED = 4003
    const val HTTP_UNAUTHORIZED = 401

    const val NOTICE_REVOKED = "This phone was removed from the server. Pair it again to keep using it as a gateway."
    const val NOTICE_UNAUTHORIZED =
        "The server no longer accepts this phone's token. Create a new pairing code in the web UI and pair again."

    /**
     * [code] is the WebSocket close code, or -1 when the connection failed; [httpStatus] is the HTTP
     * status of a refused upgrade. [ownGatewayOff] is true once this side closed with 4010.
     */
    fun onClosed(code: Int, httpStatus: Int?, stopping: Boolean, ownGatewayOff: Boolean): CloseDecision = when {
        // Checked first: a close still in flight from a stopping instance (for example during a
        // re-pair) must not wipe the new pairing. A real revoke shows up again on the next connect.
        stopping -> CloseDecision.Stay
        code == CLOSE_REVOKED -> CloseDecision.Unpair(NOTICE_REVOKED)
        httpStatus == HTTP_UNAUTHORIZED -> CloseDecision.Unpair(NOTICE_UNAUTHORIZED)
        code == CLOSE_GATEWAY_OFF && ownGatewayOff -> CloseDecision.Stay
        else -> CloseDecision.Reconnect
    }

    fun onConnect(stopping: Boolean, reconnectSuppressed: Boolean, paired: Boolean, hasToken: Boolean): ConnectDecision =
        when {
            stopping || reconnectSuppressed -> ConnectDecision.Skip
            !paired -> ConnectDecision.Shutdown
            !hasToken -> ConnectDecision.Retry
            else -> ConnectDecision.Open
        }
}
