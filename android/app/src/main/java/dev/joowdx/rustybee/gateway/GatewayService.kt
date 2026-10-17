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
import dev.joowdx.rustybee.push.Push
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.channels.Channel
import dev.joowdx.rustybee.protocol.Filters
import dev.joowdx.rustybee.protocol.Nack
import dev.joowdx.rustybee.protocol.SetForwarding
import dev.joowdx.rustybee.protocol.Ulid
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
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
    @Volatile
    private var stopping = false
    private var socketClosed = false
    /** Token already put on this connection, so a later health omits an unchanged `fcm_token`. */
    private var announcedToken: String? = null
    private var pendingForwarding: String? = null
    /** The `receive_enabled` of the save in flight. */
    private var pendingReceiveEnabled = false

    inner class LocalBinder : Binder() {
        val state: StateFlow<GatewayState> get() = stateFlow.asStateFlow()

        /** Send forwarding edited on the phone; the result shows in [GatewayState.forwardingSave]. */
        fun saveForwarding(receiveEnabled: Boolean, filters: Filters, baseRev: Long) {
            events.trySend(GatewayEvent.SaveForwarding(receiveEnabled, filters, baseRev))
        }
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
            onConfig = {
                graph.prefs.config = it
                graph.log("config updated")
                restoreForwardingOff()
                setState { s -> s.copy(filtersRev = it.filtersRev) }
            },
            onReply = ::onReply,
            log = { graph.log(it) },
        )
        Notifications.ensureChannels(this)
        Telemetry.captureLifecycle(this)
        running = true
        eventsOut = events
        instance = this
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
            val cutoff = now - 7 * 24 * 3_600_000L
            graph.db.pruneSeen(cutoff)
            graph.db.pruneSent(cutoff)
            graph.sender.failLost()
            for (ev in events) runCatching { process(ev) }.onFailure { graph.log("event failed: $it", "E") }
        }
        scope.launch {
            val before = Push.token(this@GatewayService)
            val token = Push.refresh(this@GatewayService) ?: return@launch
            if (token != before) events.trySend(GatewayEvent.FcmToken(token))
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        try {
            startForeground(
                Notifications.ID_GATEWAY,
                Notifications.gateway(this, stateFlow.value),
                ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING,
            )
        } catch (e: Exception) {
            // The start window closed before promotion, e.g. a downgraded wake; the user restarts from the notification.
            graph.log("could not promote gateway: ${e.javaClass.simpleName}", "E")
            Notifications.postStopped(this)
            stopSelf()
            return START_NOT_STICKY
        }
        val prefs = Graph.of(this).prefs
        if (!prefs.gatewayEnabled || !prefs.paired) {
            shutdown()
            return START_NOT_STICKY
        }
        connection.allowReconnect()
        events.trySend(GatewayEvent.Nudge)
        return START_STICKY
    }

    override fun onBind(intent: Intent): IBinder = binder

    override fun onDestroy() {
        running = false
        stopping = true
        if (eventsOut === events) eventsOut = null
        if (instance === this) instance = null
        runCatching { getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(networkCallback) }
        runCatching { unregisterReceiver(powerReceiver) }
        graph.uplink.detach()
        closeSocket()
        scope.cancel()
        events.close()
        super.onDestroy()
    }

    private fun shutdown() {
        stopping = true
        closeSocket()
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    /** Close once. A user switch-off (still paired, gateway disabled) uses 4010 `gateway_off`. */
    private fun closeSocket() {
        if (socketClosed) return
        socketClosed = true
        val off = graph.prefs.paired && !graph.prefs.gatewayEnabled
        connection.close(if (off) CLOSE_GATEWAY_OFF else 1000, if (off) "gateway_off" else "stopped")
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
            is GatewayEvent.FcmToken -> if (ev.token != announcedToken) sendHealth(ev.token)
            is GatewayEvent.SaveForwarding -> saveForwarding(ev.receiveEnabled, ev.filters, ev.baseRev)
            is GatewayEvent.Opened -> if (ev.gen == connection.generation) onOpened()
            is GatewayEvent.Message -> if (ev.gen == connection.generation) {
                handler.handle(ev.text) { connection.send(it) }
                publish()
            }
            is GatewayEvent.Closed -> if (ev.gen == connection.generation) onClosed(ev.code, ev.reason, ev.httpStatus)
        }
    }

    private fun connect() {
        val prefs = graph.prefs
        val paired = !stopping && prefs.paired
        val token = if (paired) prefs.token else null
        when (GatewayPolicy.onConnect(stopping, connection.reconnectSuppressed, paired, token != null)) {
            ConnectDecision.Skip -> return
            ConnectDecision.Shutdown -> {
                graph.log("not paired; stopping", "E")
                prefs.gatewayEnabled = false
                shutdown()
                return
            }
            ConnectDecision.Retry -> {
                graph.log("could not read the device token; retrying", "E")
                setState { it.copy(connection = ConnectionState.CONNECTING, lastError = "could not read the device token") }
                scheduleReconnect()
                return
            }
            ConnectDecision.Open -> Unit
        }
        reconnectJob?.cancel()
        val url = prefs.wsUrl ?: return
        if (url.startsWith("ws://") && !prefs.allowInsecure && !BuildConfig.DEBUG) {
            setState { it.copy(lastError = "insecure ws:// URL not allowed") }
            graph.log("refusing insecure ws:// URL", "E")
            return
        }
        welcomed = false
        connection.open(url, token!!, prefs.config.pingS.takeIf { it > 0 } ?: Config.DEFAULT_PING_S)
        setState { it.copy(connection = ConnectionState.CONNECTING) }
    }

    private fun onOpened() {
        backoff.onOpened(SystemClock.elapsedRealtime())
        val hello = telemetry.hello(graph.uplink.size())
        lastPermissions = hello.permissions
        announcedToken = hello.fcmToken
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
        // A token that rotated between `hello` and `welcome` was dropped by sendHealth; send it now.
        Push.token(this)?.let { if (it != announcedToken) sendHealth(it) }
        setState { it.copy(connection = ConnectionState.CONNECTED, lastError = null, filtersRev = w.config.filtersRev) }
        graph.uplink.attach { connection.send(it) }
        healthJob?.cancel()
        healthJob = scope.launch {
            while (isActive) {
                delay(15 * 60_000L)
                events.send(GatewayEvent.HealthTick)
            }
        }
        graph.log("welcome; outbox replayed")
        restoreForwardingOff()
        publish()
    }

    private fun saveForwarding(receiveEnabled: Boolean, filters: Filters, baseRev: Long) {
        // Off applies on the phone at once, even offline; on needs the server (spec 002).
        if (!receiveEnabled) graph.prefs.forwardingOffHere = true
        if (!welcomed) {
            val result = if (receiveEnabled) ForwardingSave.Failed("Not connected to the server") else ForwardingSave.SavedOnPhone
            setState { it.copy(forwardingSave = result) }
            return
        }
        // On clears the phone's own "off" only once the server accepts it, in onReply.
        val id = Ulid.next()
        pendingForwarding = id
        pendingReceiveEnabled = receiveEnabled
        sendForwarding(SetForwarding(baseRev, receiveEnabled, filters), id)
        setState { it.copy(forwardingSave = ForwardingSave.Saving) }
    }

    /** Tell the server about an "off" set on this phone that its config doesn't show yet. */
    private fun restoreForwardingOff() {
        if (!welcomed || pendingForwarding != null) return
        val frame = Forwarding.restore(graph.prefs.config, graph.prefs.forwardingOffHere) ?: return
        graph.log("forwarding is off on this phone; telling the server")
        sendForwarding(frame, Ulid.next())
    }

    private fun sendForwarding(f: SetForwarding, id: String) =
        connection.send(Frames.encode(FrameType.SET_FORWARDING, SetForwarding.serializer(), f, id))

    private fun onReply(ref: String, nack: Nack?) {
        if (ref != pendingForwarding) return
        pendingForwarding = null
        if (nack == null && pendingReceiveEnabled) graph.prefs.forwardingOffHere = false
        val result = when {
            nack == null -> ForwardingSave.Saved
            nack.code == "filters_conflict" -> ForwardingSave.Conflict
            else -> ForwardingSave.Failed(nack.message ?: nack.code)
        }
        graph.log("forwarding save: ${result.javaClass.simpleName}")
        setState { it.copy(forwardingSave = result) }
    }

    private fun onClosed(code: Int, reason: String, httpStatus: Int?) {
        if (pendingForwarding != null) {
            pendingForwarding = null
            setState { it.copy(forwardingSave = ForwardingSave.Failed("Connection lost before the server answered")) }
        }
        graph.uplink.detach()
        healthJob?.cancel()
        welcomed = false
        backoff.onClosed(SystemClock.elapsedRealtime())
        connection.close()
        graph.log("socket closed ($code $reason)")
        when (val d = GatewayPolicy.onClosed(code, httpStatus, stopping, connection.reconnectSuppressed)) {
            is CloseDecision.Unpair -> unpairHere(if (code == GatewayPolicy.CLOSE_REVOKED) "revoked" else "unauthorized", d.notice)
            CloseDecision.Stay -> setState { it.copy(connection = ConnectionState.STOPPED, lastError = null) }
            CloseDecision.Reconnect -> {
                setState { it.copy(connection = ConnectionState.CONNECTING, lastError = "$code $reason") }
                scheduleReconnect()
            }
        }
    }

    /** AC-002-07 / AC-002-18: the server dropped this device. Wipe, stop, alert and leave Setup to the UI. */
    private fun unpairHere(why: String, notice: String) {
        graph.log("server says $why; wiping credentials", "E")
        graph.prefs.clearPairing(notice)
        graph.db.wipe()
        Watchdog.cancel(this)
        setState { it.copy(connection = ConnectionState.STOPPED, lastError = why) }
        Notifications.postUnpaired(this, notice)
        shutdown()
    }

    private fun scheduleReconnect() {
        val wait = backoff.nextDelayMs()
        reconnectJob?.cancel()
        reconnectJob = scope.launch {
            delay(wait)
            events.send(GatewayEvent.Connect)
        }
    }

    private fun sendHealth(fcmToken: String? = null) {
        // Before `welcome` the hello (built from prefs) carries the token, and a health must not precede it.
        if (!welcomed) return
        val token = fcmToken?.takeIf { it != announcedToken }
        val health = telemetry.health(graph.uplink.size(), token)
        lastPermissions = health.permissions
        if (connection.send(Frames.encode(FrameType.HEALTH, Health.serializer(), health)) && token != null) {
            announcedToken = token
        }
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
                filtersRev = graph.prefs.config.filtersRev,
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

        @Volatile
        var running = false
            private set

        @Volatile
        private var eventsOut: Channel<GatewayEvent>? = null

        @Volatile
        private var instance: GatewayService? = null

        /** Starts the service or nudges a running one. False when the start is refused, for example in the background. */
        fun start(context: Context): Boolean =
            try {
                context.startForegroundService(Intent(context, GatewayService::class.java))
                true
            } catch (e: Exception) {
                Graph.of(context).log("could not start gateway: ${e.javaClass.simpleName}", "E")
                false
            }

        /** Stops the service. It stops connecting at once, before `onDestroy` runs (AC-013-04, re-pairing). */
        fun stop(context: Context) {
            instance?.stopping = true
            context.stopService(Intent(context, GatewayService::class.java))
        }

        /** If the socket is open, send a health frame carrying [token]. */
        fun onFcmToken(token: String) {
            eventsOut?.trySend(GatewayEvent.FcmToken(token))
        }
    }
}
