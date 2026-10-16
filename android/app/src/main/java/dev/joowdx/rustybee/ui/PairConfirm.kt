package dev.joowdx.rustybee.ui

/** Pure pairing decisions for `SetupActivity` (spec 013, AC-013-07). */
object PairConfirm {
    /**
     * The link an incoming intent (deep link, `onNewIntent`, scan result) puts in the field, or null.
     * An intent never pairs on its own: pairing always needs the Pair button and the confirm dialog.
     */
    fun linkFromIntent(data: String?): String? =
        data?.takeIf { it.startsWith("rustybee:", ignoreCase = true) }

    /** Incoming intents only fill the field. */
    const val AUTO_PAIR_FROM_INTENT = false

    fun title(paired: Boolean): String = if (paired) "Replace the current server?" else "Pair with this server?"

    fun message(host: String, paired: Boolean): String =
        if (paired) {
            "Pair with $host? This replaces the current server and wipes this phone's local rustybee data " +
                "(outbox, send history and log)."
        } else {
            "Pair with $host? Only continue if you trust this server: it will be able to send SMS from this phone " +
                "and receive the SMS it gets."
        }
}
