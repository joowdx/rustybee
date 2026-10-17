package dev.joowdx.rustybee.gateway

import dev.joowdx.rustybee.protocol.Filters
import kotlinx.coroutines.channels.SendChannel
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.util.concurrent.TimeUnit

/** App-initiated close when the user switches the gateway off (spec 002). */
const val CLOSE_GATEWAY_OFF = 4010

/** One OkHttp WebSocket at a time. Callbacks carry a generation so stale sockets are ignored. */
class Connection(private val events: SendChannel<GatewayEvent>) {
    private val base = OkHttpClient.Builder()
        .connectTimeout(15, TimeUnit.SECONDS)
        .readTimeout(0, TimeUnit.MILLISECONDS)
        .build()

    @Volatile
    var generation = 0
        private set

    @Volatile
    private var socket: WebSocket? = null

    /** Set when this side closes with [CLOSE_GATEWAY_OFF]. [open] then refuses until [allowReconnect]. */
    @Volatile
    var reconnectSuppressed = false
        private set

    var pingSeconds = 0
        private set

    val isOpen: Boolean get() = socket != null

    fun allowReconnect() {
        reconnectSuppressed = false
    }

    fun open(url: String, token: String, pingS: Int): Int {
        if (reconnectSuppressed) return generation
        close(1000, "reconnect")
        val gen = ++generation
        pingSeconds = pingS
        val client = base.newBuilder().pingInterval(pingS.toLong(), TimeUnit.SECONDS).build()
        val request = Request.Builder().url(url).header("Authorization", "Bearer $token").build()
        socket = client.newWebSocket(request, Listener(gen))
        return gen
    }

    fun send(text: String): Boolean = socket?.send(text) ?: false

    fun close(code: Int = 1000, reason: String? = null) {
        val current = socket
        if (current != null && code == CLOSE_GATEWAY_OFF) reconnectSuppressed = true
        current?.let { if (!it.close(code, reason)) it.cancel() }
        socket = null
        generation++
    }

    private inner class Listener(private val gen: Int) : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            events.trySend(GatewayEvent.Opened(gen))
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            events.trySend(GatewayEvent.Message(gen, text))
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
            events.trySend(GatewayEvent.Closed(gen, code, reason))
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            val why = response?.let { "HTTP ${it.code}" } ?: (t.message ?: t.javaClass.simpleName)
            events.trySend(GatewayEvent.Closed(gen, -1, why, response?.code))
        }
    }
}

sealed interface GatewayEvent {
    data class Opened(val gen: Int) : GatewayEvent
    data class Message(val gen: Int, val text: String) : GatewayEvent
    /** [httpStatus] is set when the upgrade itself was refused, e.g. 401 for a token the server rejects. */
    data class Closed(val gen: Int, val code: Int, val reason: String, val httpStatus: Int? = null) : GatewayEvent
    data object Connect : GatewayEvent
    data object NetworkChanged : GatewayEvent
    /** The default network's transport, read from its capabilities callback. */
    data class NetworkType(val label: String) : GatewayEvent
    /** A Network switch changed (spec 015): re-register listeners, then check. */
    data object RadioSettings : GatewayEvent
    /** A Wi-Fi reading changed, or a held-back bar change is due. */
    data object RadiosChanged : GatewayEvent
    /** The data SIM's signal or service state changed; re-read it. */
    data object CellularChanged : GatewayEvent
    data object Nudge : GatewayEvent
    data object HealthTick : GatewayEvent
    data class FcmToken(val token: String) : GatewayEvent
    data class SaveForwarding(val receiveEnabled: Boolean, val filters: Filters, val baseRev: Long) : GatewayEvent
}
