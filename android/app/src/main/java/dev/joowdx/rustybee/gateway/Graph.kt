package dev.joowdx.rustybee.gateway

import android.content.Context
import android.util.Log
import dev.joowdx.rustybee.data.Db
import dev.joowdx.rustybee.data.Prefs

/** Process-wide singletons shared by the service, receivers and UI. */
class Graph private constructor(context: Context) {
    val app: Context = context.applicationContext
    val prefs = Prefs(app)
    val db = Db(app)
    val uplink = Uplink(db)
    val sender = Sender(
        sent = db,
        uplink = uplink,
        radio = AndroidSmsRadio(app, prefs),
        configSim = { prefs.config.sim },
        log = { log(it) },
    )

    fun log(msg: String, level: String = "I") {
        Log.println(if (level == "E") Log.ERROR else Log.INFO, "rustybee", Redact.text(msg))
        runCatching { db.log(level, msg) }
    }

    companion object {
        @Volatile
        private var instance: Graph? = null

        fun of(context: Context): Graph =
            instance ?: synchronized(this) { instance ?: Graph(context).also { instance = it } }
    }
}
