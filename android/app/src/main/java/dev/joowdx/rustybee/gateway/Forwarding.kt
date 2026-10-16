package dev.joowdx.rustybee.gateway

import dev.joowdx.rustybee.protocol.Config
import dev.joowdx.rustybee.protocol.SetForwarding

/** The phone has the final say on forwarding (spec 002). */
object Forwarding {
    /** Whether received texts are forwarded at all; the filters apply only when this is true. */
    fun on(config: Config, offHere: Boolean): Boolean = config.receiveEnabled && !offHere

    /** The `set_forwarding` that brings the server back in line with an "off" set on this phone. */
    fun restore(config: Config, offHere: Boolean): SetForwarding? =
        if (offHere && config.receiveEnabled) SetForwarding(config.filtersRev, false, config.filters) else null
}
