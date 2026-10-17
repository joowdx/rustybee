package dev.joowdx.rustybee.gateway

import dev.joowdx.rustybee.protocol.StatusError

/** Spec 012 error mapping. Numeric codes mirror `SmsManager.RESULT_*` and `Activity.RESULT_OK`. */
object SmsErrors {
    const val RESULT_OK = -1
    const val GENERIC_FAILURE = 1
    const val RADIO_OFF = 2
    const val NULL_PDU = 3
    const val NO_SERVICE = 4
    const val LIMIT_EXCEEDED = 5
    const val SHORT_CODE_NOT_ALLOWED = 7
    const val SHORT_CODE_NEVER_ALLOWED = 8

    fun fromResult(code: Int): StatusError = when (code) {
        GENERIC_FAILURE -> StatusError("generic_failure", "Generic failure")
        RADIO_OFF -> StatusError("radio_off", "Radio is off")
        NULL_PDU -> StatusError("null_pdu", "No PDU provided")
        NO_SERVICE -> StatusError("no_service", "No cellular service")
        LIMIT_EXCEEDED -> StatusError("limit_exceeded", "Sending limit exceeded")
        SHORT_CODE_NOT_ALLOWED -> StatusError("short_code_not_allowed", "Short code not allowed")
        SHORT_CODE_NEVER_ALLOWED -> StatusError("short_code_never_allowed", "Short code never allowed")
        else -> StatusError("unknown_$code", "Unknown error $code")
    }

    val permissionDenied = StatusError("permission_denied", "SMS permission denied")
    val phoneStateDenied = StatusError("permission_denied", "Phone state permission denied; cannot check the requested SIM")
    val invalidDestination = StatusError("invalid_destination", "Invalid destination or body")
    val simUnavailable = StatusError("sim_unavailable", "Requested SIM is not active")
    val deliveryFailed = StatusError("delivery_failed", "Carrier reported delivery failure")
    fun radioFailed(e: RuntimeException) = StatusError("generic_failure", "Radio call failed: ${e.javaClass.simpleName}")
    val outcomeLost = StatusError("generic_failure", "outcome lost")
}
