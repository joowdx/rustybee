package dev.joowdx.rustybee.gateway

import android.app.NotificationManager
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.os.Binder
import android.os.IBinder
import android.os.SystemClock
import dev.joowdx.rustybee.BuildConfig
import dev.joowdx.rustybee.protocol.Config
import dev.joowdx.rustybee.protocol.FrameType
import dev.joowdx.rustybee.protocol.Frames
import dev.joowdx.rustybee.protocol.Health
import dev.joowdx.rustybee.protocol.Hello
import dev.joowdx.rustybee.protocol.Permissions
import dev.joowdx.rustybee.protocol.Welcome
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/**
 * The `remoteMessaging` foreground service (spec 011). All socket, send and uplink work is
 * processed in one coroutine fed by [events].
 */
class GatewayService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val events = Channel<GatewayEvent>(Channel.UNLIMITED)
    private val connection = Connection(events)
    private val backoff = Backoff()
    private val stateFlow = MutableStateFlow(GatewayState())

    private lateinit var graph: Graph
    private lateinit var telemetry: Telemetry
    private lateinit var handler: FrameHandler

    private var reconnectJob: Job? = null
    private var healthJob: Job? = null
    private var welcomed = false
    private var lastNetwork: Network? = null
    private var lastPermissions: Permissions? = null
    private var stopping = false

    inner class LocalBinder : Binder() {
        val state: StateFlow<GatewayState> get() = stateFlow.asStateFlow()
    }

    private val binder = LocalBinder()

    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            if (network != lastNetwork) {
                val first = lastNetwork == null
                lastNetwork = network
                if (!first) events.trySend(GatewayEvent.NetworkChanged)
            }
        }

        override fun onLost(network: Network) {
            if (network == lastNetwork) lastNetwork = null
            events.trySend(GatewayEvent.NetworkChanged)
        }
    }

    private val powerReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            events.trySend(GatewayEvent.HealthTick)
        }
    }

    override fun onCreate() {
        super.onCreate()
        graph = Graph.of(this)
        telemetry = Telemetry(this)
        handler = FrameHandler(
            seen = graph.db,
            uplink = graph.uplink,
            sender = graph.sender,
            onWelcome = ::onWelcome,
            onConfig = { graph.prefs.config = it; graph.log("config updated") },
            log = { graph.log(it) },
        )
        Notifications.ensureChannels(this)
        running = true
        startForeground(
            Notifications.ID_GATEWAY,
            Notifications.gateway(this, stateFlow.value),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING,
        )
        getSystemService(ConnectivityManager::class.java).registerDefaultNetworkCallback(networkCallback)
        registerReceiver(
            powerReceiver,
            IntentFilter().apply {
                addAction(Intent.ACTION_POWER_CONNECTED)
                addAction(Intent.ACTION_POWER_DISCONNECTED)
            },
            RECEIVER_NOT_EXPORTED,
        )
        scope.launch {
            val now = System.currentTimeMillis()
            graph.db.pruneSeen(now - 7 * 24 * 3_600_000L)
            graph.sender.failLost()
            for (ev in events) runCatching { process(ev) }.onFailure { graph.log("event failed: $it", "E") }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val prefs = Graph.of(this).prefs
        if (!prefs.gatewayEnabled || !prefs.paired) {
            shutdown()
            return START_NOT_STICKY
        }
        events.trySend(GatewayEvent.Nudge)
        return START_STICKY
    }

    override fun onBind(intent: Intent): IBinder = binder

    override fun onDestroy() {
        running = false
        runCatching { getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(networkCallback) }
        runCatching { unregisterReceiver(powerReceiver) }
        graph.uplink.detach()
        connection.close(1000, "stopped")
        scope.cancel()
        events.close()
        super.onDestroy()
    }

    private fun shutdown() {
        stopping = true
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    // --- event loop (single coroutine)

    private fun process(ev: GatewayEvent) {
        when (ev) {
            GatewayEvent.Connect -> if (stateFlow.value.connection != ConnectionState.CONNECTED) connect()
            GatewayEvent.Nudge -> {
                if (stateFlow.value.connection == ConnectionState.STOPPED) connect()
                maybeSendHealthOnPermissionChange()
                publish()
            }
            GatewayEvent.NetworkChanged -> {
                graph.log("network changed: reconnecting")
                backoff.reset()
                connect()
            }
            GatewayEvent.HealthTick -> sendHealth()
            is GatewayEvent.Opened -> if (ev.gen == connection.generation) onOpened()
            is GatewayEvent.Message -> if (ev.gen == connection.generation) {
                handler.handle(ev.text) { connection.send(it) }
                publish()
            }
            is GatewayEvent.Closed -> if (ev.gen == connection.generation) onClosed(ev.code, ev.reason)
        }
    }

    private fun connect() {
        if (stopping) return
        reconnectJob?.cancel()
        val prefs = graph.prefs
        val url = prefs.wsUrl
        val token = prefs.token
        if (url == null || token == null) {
            graph.log("not paired; stopping", "E")
            prefs.gatewayEnabled = false
            shutdown()
            return
        }
        if (url.startsWith("ws://") && !prefs.allowInsecure && !BuildConfig.DEBUG) {
            setState { it.copy(lastError = "insecure ws:// URL not allowed") }
            graph.log("refusing insecure ws:// URL", "E")
            return
        }
        welcomed = false
        connection.open(url, token, prefs.config.pingS.takeIf { it > 0 } ?: Config.DEFAULT_PING_S)
        setState { it.copy(connection = ConnectionState.CONNECTING) }
    }

    private fun onOpened() {
        backoff.onOpened(SystemClock.elapsedRealtime())
        val hello = telemetry.hello(graph.uplink.size())
        lastPermissions = hello.permissions
        connection.send(Frames.encode(FrameType.HELLO, Hello.serializer(), hello))
        graph.log("socket open; hello sent")
    }

    private fun onWelcome(w: Welcome) {
        val prefs = graph.prefs
        prefs.config = w.config
        prefs.deviceId = w.deviceId
        prefs.lastConnectedAt = System.currentTimeMillis()
        if (w.config.pingS > 0 && w.config.pingS != connection.pingSeconds) {
            graph.log("ping interval ${connection.pingSeconds}s -> ${w.config.pingS}s; reconnecting")
            connect()
            return
        }
        welcomed = true
        setState { it.copy(connection = ConnectionState.CONNECTED, lastError = null) }
        graph.uplink.attach { connection.send(it) }
        healthJob?.cancel()
        healthJob = scope.launch {
            while (isActive) {
                delay(15 * 60_000L)
                events.send(GatewayEvent.HealthTick)
            }
        }
        graph.log("welcome; outbox replayed")
        publish()
    }

    private fun onClosed(code: Int, reason: String) {
        graph.uplink.detach()
        healthJob?.cancel()
        welcomed = false
        backoff.onClosed(SystemClock.elapsedRealtime())
        connection.close()
        graph.log("socket closed ($code $reason)")
        if (code == CLOSE_REVOKED) {
            graph.log("device revoked by server; wiping credentials", "E")
            graph.prefs.clearPairing()
            graph.db.wipe()
            Watchdog.cancel(this)
            setState { it.copy(connection = ConnectionState.STOPPED, lastError = "revoked") }
            shutdown()
            return
        }
        setState { it.copy(connection = ConnectionState.CONNECTING, lastError = "$code $reason") }
        val wait = backoff.nextDelayMs()
        reconnectJob?.cancel()
        reconnectJob = scope.launch {
            delay(wait)
            events.send(GatewayEvent.Connect)
        }
    }

    private fun sendHealth() {
        if (!welcomed) return
        val health = telemetry.health(graph.uplink.size())
        lastPermissions = health.permissions
        connection.send(Frames.encode(FrameType.HEALTH, Health.serializer(), health))
    }

    private fun maybeSendHealthOnPermissionChange() {
        if (welcomed && telemetry.permissions() != lastPermissions) sendHealth()
    }

    private fun publish() = setState { it }

    private fun setState(f: (GatewayState) -> GatewayState) {
        stateFlow.update {
            f(it).copy(
                outbox = graph.uplink.size(),
                sendsToday = graph.prefs.today("sends"),
                receivesToday = graph.prefs.today("receives"),
            )
        }
        if (!stopping) {
            runCatching {
                getSystemService(NotificationManager::class.java)
                    .notify(Notifications.ID_GATEWAY, Notifications.gateway(this, stateFlow.value))
            }
        }
    }

    companion object {
        const val CLOSE_REVOKED = 4003

        @Volatile
        var running = false
            private set

        /** Starts the service or nudges a running one; failures (background limits) are logged. */
        fun start(context: Context) {
            try {
                context.startForegroundService(Intent(context, GatewayService::class.java))
            } catch (e: Exception) {
                Graph.of(context).log("could not start gateway: ${e.javaClass.simpleName}", "E")
            }
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, GatewayService::class.java))
        }
    }
}
