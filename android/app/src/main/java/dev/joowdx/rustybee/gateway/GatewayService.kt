package dev.joowdx.rustybee.gateway

import android.Manifest
import android.annotation.SuppressLint
import android.annotation.TargetApi
import android.app.NotificationManager
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Binder
import android.os.Build
import android.os.IBinder
import android.os.SystemClock
import android.telephony.SignalStrength
import android.telephony.SubscriptionManager
import android.telephony.TelephonyDisplayInfo
import android.telephony.TelephonyManager
import dev.joowdx.rustybee.BuildConfig
import dev.joowdx.rustybee.protocol.Config
import dev.joowdx.rustybee.protocol.Filters
import dev.joowdx.rustybee.protocol.FrameType
import dev.joowdx.rustybee.protocol.Frames
import dev.joowdx.rustybee.protocol.Health
import dev.joowdx.rustybee.protocol.Hello
import dev.joowdx.rustybee.protocol.Nack
import dev.joowdx.rustybee.protocol.Permissions
import dev.joowdx.rustybee.protocol.Radios
import dev.joowdx.rustybee.protocol.SetForwarding
import dev.joowdx.rustybee.protocol.SetSim
import dev.joowdx.rustybee.protocol.SimInfo
import dev.joowdx.rustybee.protocol.Ulid
import dev.joowdx.rustybee.protocol.Welcome
import dev.joowdx.rustybee.push.Push
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
    /** `network` as last sent on this connection, and as last seen by the network callback. */
    private var reportedNetwork: String? = null
    private var currentNetwork: String? = null
    /** Spec 015: latest readings from the listeners, and what was last sent with when. */
    @Volatile
    private var wifi: Pair<Network, RadioReading>? = null
    @Volatile
    private var cellSignal: SignalStrength? = null
    @Volatile
    private var cellDisplay: TelephonyDisplayInfo? = null
    /** Read on cellular events only, so Wi-Fi updates don't cost Telephony calls. */
    private var cellReading: RadioReading? = null
    /** The data subscription the cellular listener is registered on. */
    private var cellSub: Int? = null
    private var reportedRadios: Radios? = null
    private var lastReportAt: Long? = null
    private var radioJob: Job? = null
    /** When [radioJob] fires, and until when newly started listeners hold reports back. */
    private var radioDueAt = 0L
    private var settleUntil = 0L
    /** Whether [wifiCallback] asked for location info. */
    private var wifiWithName = false
    /** Volatile: callbacks compare themselves to these to drop stale events. */
    @Volatile
    private var wifiCallback: ConnectivityManager.NetworkCallback? = null
    @Volatile
    private var cellCallback: TelephonyWatch? = null
    @Volatile
    private var displayCallback: TelephonyWatch? = null
    @Volatile
    private var stopping = false
    private var socketClosed = false
    /** Token already put on this connection, so a later health omits an unchanged `fcm_token`. */
    private var announcedToken: String? = null
    private var pendingForwarding: String? = null
    /** The `receive_enabled` of the save in flight. */
    private var pendingReceiveEnabled = false
    /** Frame id of the `set_sim` in flight. */
    private var pendingSim: String? = null
    /** A `set_sim` whose reply was lost with the connection; the next config says whether it was stored. */
    private var unconfirmedSim: SetSim? = null
    /** The SIM list as last sent on this connection; the listener also fires without a change. */
    private var reportedSims: List<SimInfo>? = null
    /** The SIM list as last seen by the listener, to redraw an open main screen. */
    private var seenSims: List<SimInfo>? = null
    /** What the posted gateway notification shows, so unchanged state is not re-posted. */
    @Volatile
    private var notified: Notifications.GatewayContent? = null

    inner class LocalBinder : Binder() {
        val state: StateFlow<GatewayState> get() = stateFlow.asStateFlow()

        /** Send forwarding edited on the phone; the result shows in [GatewayState.forwardingSave]. */
        fun saveForwarding(receiveEnabled: Boolean, filters: Filters, baseRev: Long) {
            events.trySend(GatewayEvent.SaveForwarding(receiveEnabled, filters, baseRev))
        }

        /** Send the preferred SIM picked on the phone; null is the phone's default. Needs a live connection. */
        fun saveSim(sim: Int?) {
            events.trySend(GatewayEvent.SaveSim(sim))
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

        // A transport change keeps the same default network when it happens under a VPN, so no
        // onAvailable fires and the socket survives; only the capabilities say what changed.
        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
            events.trySend(GatewayEvent.NetworkType(Telemetry.networkLabel(caps::hasTransport)))
        }

        override fun onLost(network: Network) {
            if (network == lastNetwork) lastNetwork = null
            events.trySend(GatewayEvent.NetworkChanged)
        }
    }

    private val subscriptionsListener = object : SubscriptionManager.OnSubscriptionsChangedListener() {
        override fun onSubscriptionsChanged() {
            events.trySend(GatewayEvent.SimsChanged)
        }
    }

    /** A SIM turned on or off, or airplane mode, changes the usable SIMs without a subscription change. */
    private val simReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            events.trySend(GatewayEvent.SimsChanged)
        }
    }

    /** One radio listener per active subscription, so a SIM turned off is seen whichever it is. */
    private val simWatch = mutableMapOf<Int, TelephonyWatch>()

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
                val simChanged = it.sim != graph.prefs.config.sim
                graph.prefs.config = it
                confirmSim(it.sim)
                graph.log("config updated")
                restoreForwardingOff()
                setState { s -> s.copy(filtersRev = it.filtersRev, simRev = s.simRev + simChanged.compareTo(false)) }
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
        watchRadios()
        // A SIM removed or added reaches the server without a reconnect; the list needs READ_PHONE_STATE.
        runCatching {
            getSystemService(SubscriptionManager::class.java).addOnSubscriptionsChangedListener(mainExecutor, subscriptionsListener)
        }.onFailure { graph.log("SIM listener failed: $it", "E") }
        registerPrivateReceiver(
            simReceiver,
            IntentFilter().apply {
                addAction(ACTION_SIM_STATE_CHANGED)
                addAction(Intent.ACTION_AIRPLANE_MODE_CHANGED)
            },
        )
        watchSims()
        registerPrivateReceiver(
            powerReceiver,
            IntentFilter().apply {
                addAction(Intent.ACTION_POWER_CONNECTED)
                addAction(Intent.ACTION_POWER_DISCONNECTED)
            },
        )
        scope.launch {
            maintenance()
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
            val state = stateFlow.value
            // Android 11 has no remoteMessaging type and needs none (spec 011).
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                startForeground(
                    Notifications.ID_GATEWAY,
                    Notifications.gateway(this, state),
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING,
                )
            } else {
                startForeground(Notifications.ID_GATEWAY, Notifications.gateway(this, state))
            }
            notified = Notifications.content(state)
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
        runCatching { unregisterReceiver(simReceiver) }
        unwatchSims(simWatch.keys.toSet())
        runCatching { getSystemService(SubscriptionManager::class.java).removeOnSubscriptionsChangedListener(subscriptionsListener) }
        unwatchWifi()
        unwatchCellular()
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
                // A permission granted since (e.g. READ_PHONE_STATE) may enable a listener.
                watchRadios()
                maybeSendHealthOnPermissionChange()
                publish()
            }
            GatewayEvent.NetworkChanged -> {
                graph.log("network changed: reconnecting")
                // The new network's transport follows in its own NetworkType event.
                currentNetwork = null
                backoff.reset()
                connect()
            }
            GatewayEvent.HealthTick -> {
                maintenance()
                sendHealth()
            }
            is GatewayEvent.NetworkType -> {
                // Capabilities change often on one network; only a transport change re-reads the
                // cellular generation, which shows only on mobile data.
                val changed = ev.label != currentNetwork
                currentNetwork = ev.label
                if (changed) readCellular()
                if (GatewayPolicy.reportNetwork(welcomed, reportedNetwork, currentNetwork)) {
                    sendHealth(fresh = changed)
                } else if (changed) {
                    checkRadios()
                }
            }
            GatewayEvent.RadioSettings -> {
                watchRadios()
                readCellular()
                checkRadios()
            }
            // A Wi-Fi reading changed, or a held-back report is due; cellular is kept current by its own events.
            GatewayEvent.RadiosChanged -> checkRadios()
            GatewayEvent.CellularChanged -> {
                readCellular()
                checkRadios()
            }
            is GatewayEvent.FcmToken -> if (ev.token != announcedToken) sendHealth(ev.token)
            is GatewayEvent.SaveForwarding -> saveForwarding(ev.receiveEnabled, ev.filters, ev.baseRev)
            is GatewayEvent.SaveSim -> saveSim(ev.sim)
            GatewayEvent.SimsChanged -> onSimsChanged()
            is GatewayEvent.Opened -> if (ev.gen == connection.generation) onOpened()
            is GatewayEvent.Message -> if (ev.gen == connection.generation) {
                handler.handle(ev.text) { connection.send(it) }
                publish()
            }
            is GatewayEvent.Closed -> if (ev.gen == connection.generation) onClosed(ev.code, ev.reason, ev.httpStatus)
        }
    }

    /** Spec 011/012: prune `seen` and final `sent` rows after 7 days, fail lost sends. At start and on every health tick. */
    private fun maintenance() {
        val cutoff = System.currentTimeMillis() - 7 * 24 * 3_600_000L
        runCatching {
            graph.db.pruneSeen(cutoff)
            graph.db.pruneSent(cutoff)
            graph.sender.failLost()
        }.onFailure { graph.log("maintenance failed: $it", "E") }
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
        if (!Cleartext.allowed(url)) {
            // AC-011-12: checked again here, so a stored or edited URL can't reach a public host in cleartext.
            setState { it.copy(connection = ConnectionState.STOPPED, lastError = "cleartext ws:// to a public host refused") }
            graph.log("refusing cleartext ws:// to a non-LAN host", "E")
            return
        }
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
        readCellular()
        val hello = telemetry.hello(graph.uplink.size(), currentRadios())
        lastPermissions = hello.permissions
        announcedToken = hello.fcmToken
        if (connection.send(Frames.encode(FrameType.HELLO, Hello.serializer(), hello))) {
            reportedRadios = hello.radios
            lastReportAt = SystemClock.elapsedRealtime()
            reportedNetwork = hello.network
            reportedSims = hello.sims
        }
        graph.log("socket open; hello sent")
    }

    private fun onWelcome(w: Welcome) {
        val prefs = graph.prefs
        prefs.config = w.config
        confirmSim(w.config.sim)
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
        // The hello read the active network while it may have been switching, and readings that
        // arrived before `welcome` were held back.
        maybeSendHealthOnNetworkChange()
        checkRadios()
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

    /**
     * The server may have stored a `set_sim` whose reply was lost. Once a config shows the requested
     * SIM, the "connection lost" error is wrong and is cleared; a different SIM keeps it.
     */
    private fun confirmSim(configSim: Int?) {
        val asked = unconfirmedSim ?: return
        if (pendingSim != null) return
        unconfirmedSim = null
        if (asked.sim == configSim) setState { it.copy(simError = null, simRev = it.simRev + 1) }
    }

    /** Spec 002 `set_sim`: the server answers with an ack and a `config` carrying the new SIM, applied by onConfig. */
    private fun saveSim(sim: Int?) {
        if (!welcomed) {
            setState { it.copy(simError = "Not connected to the server") }
            return
        }
        val id = Ulid.next()
        if (!connection.send(Frames.encode(FrameType.SET_SIM, SetSim.serializer(), SetSim(sim), id))) {
            setState { it.copy(simError = "Not connected to the server") }
            return
        }
        pendingSim = id
        unconfirmedSim = SetSim(sim)
        setState { it.copy(simError = null) }
    }

    private fun sendForwarding(f: SetForwarding, id: String) =
        connection.send(Frames.encode(FrameType.SET_FORWARDING, SetForwarding.serializer(), f, id))

    private fun onReply(ref: String, nack: Nack?) {
        if (ref == pendingSim) {
            pendingSim = null
            unconfirmedSim = null
            if (nack != null) graph.log("preferred SIM not saved: ${nack.code}", "E")
            // Always redraw: a nack with the same text as the last one must still reset the choice.
            setState { it.copy(simError = nack?.let { n -> n.message ?: n.code }, simRev = it.simRev + 1) }
            return
        }
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
        if (pendingSim != null) {
            pendingSim = null
            setState { it.copy(simError = "Connection lost before the server answered", simRev = it.simRev + 1) }
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

    /** [fresh]: the cellular reading was just taken, so it is not read again. */
    /** The listener also fires without a change, so only a different list redraws or reports. */
    private fun onSimsChanged() {
        watchSims()
        val now = telemetry.readableSims() ?: return
        if (now != seenSims) {
            seenSims = now
            setState { it.copy(simRev = it.simRev + 1) }
        }
        if (welcomed && now != reportedSims) sendHealth(sims = now)
    }

    /** Both receivers take system broadcasts only; the not-exported flag exists from API 33. */
    private fun registerPrivateReceiver(receiver: BroadcastReceiver, filter: IntentFilter) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            registerReceiver(receiver, filter, RECEIVER_NOT_EXPORTED)
        } else {
            registerReceiver(receiver, filter)
        }
    }

    @SuppressLint("MissingPermission")
    private fun watchSims() {
        if (checkSelfPermission(Manifest.permission.READ_PHONE_STATE) != PackageManager.PERMISSION_GRANTED) return
        val subs = runCatching {
            getSystemService(SubscriptionManager::class.java).activeSubscriptionInfoList.orEmpty().map { it.subscriptionId }.toSet()
        }.getOrDefault(emptySet())
        unwatchSims(simWatch.keys - subs)
        val tm = getSystemService(TelephonyManager::class.java)
        for (sub in subs - simWatch.keys) {
            val cb = TelephonyWatch(TelephonyWatch.Watch.SERVICE_STATE) { _, _ -> events.trySend(GatewayEvent.SimsChanged) }
            runCatching { cb.start(tm.createForSubscriptionId(sub), mainExecutor) }
                .onSuccess { simWatch[sub] = cb }
                .onFailure { graph.log("SIM radio listener failed: $it", "E") }
        }
    }

    private fun unwatchSims(subs: Set<Int>) {
        for (sub in subs) simWatch.remove(sub)?.stop()
    }

    private fun sendHealth(fcmToken: String? = null, fresh: Boolean = false, sims: List<SimInfo>? = telemetry.readableSims()) {
        // Before `welcome` the hello (built from prefs) carries the token, and a health must not precede it.
        if (!welcomed) return
        val token = fcmToken?.takeIf { it != announcedToken }
        if (!fresh) readCellular()
        val health = telemetry.health(graph.uplink.size(), token, network(), currentRadios(), sims)
        lastPermissions = health.permissions
        if (connection.send(Frames.encode(FrameType.HEALTH, Health.serializer(), health))) {
            reportedRadios = health.radios
            lastReportAt = SystemClock.elapsedRealtime()
            reportedNetwork = health.network
            health.sims?.let { reportedSims = it }
            if (token != null) announcedToken = token
        }
    }

    // --- network details (spec 015)

    /** The `network` value to report: the callback's latest, or a fresh read while it is unknown. */
    private fun network(): String = currentNetwork ?: telemetry.network()

    private fun currentRadios(): Radios {
        val sw = graph.prefs.radioSwitches
        return RadioReport.build(
            sw,
            cellular = if (sw.cellular) cellReading else null,
            wifi = if (sw.wifi) wifi?.second else null,
        )
    }

    /** Re-reads the data SIM, moving the listener over when the data SIM changed. */
    private fun readCellular() {
        if (!graph.prefs.radioSwitches.cellular) {
            cellReading = null
            return
        }
        if (cellCallback != null && cellSub != SubscriptionManager.getDefaultDataSubscriptionId()) {
            unwatchCellular()
            watchRadios()
        }
        val signal = graph.prefs.radioSwitches.cellularSignal
        cellReading = runCatching { telemetry.cellularReading(cellSignal, cellDisplay, network(), signal) }.getOrNull()
    }

    private fun checkRadios() {
        val now = SystemClock.elapsedRealtime()
        val since = lastReportAt?.let { now - it }
        when (val d = RadioReport.decide(welcomed, reportedRadios, currentRadios(), since, settleUntil - now)) {
            RadioDecision.Skip -> Unit
            RadioDecision.Now -> sendHealth(fresh = true)
            // A sooner check (a settle) replaces a later one (the minute's wait), never the reverse.
            is RadioDecision.Later -> if (radioJob?.isActive != true || now + d.delayMs < radioDueAt) {
                radioJob?.cancel()
                radioDueAt = now + d.delayMs
                radioJob = scope.launch {
                    delay(d.delayMs)
                    events.send(GatewayEvent.RadiosChanged)
                }
            }
        }
    }

    /** Listeners run only while a switch that needs them is on. */
    private fun watchRadios() {
        val sw = graph.prefs.radioSwitches
        // Location info (the Wi-Fi name) is asked for only while the name is reported, so the signal
        // alone never counts as a location access.
        if (wifiCallback != null && (!sw.wifi || wifiWithName != sw.wifiName)) unwatchWifi()
        if (sw.wifi && wifiCallback == null) {
            // Location info is only included with this flag, and only shown with the location permissions.
            // Android 11 has no flag: there the name comes from WifiManager (Telemetry.wifiReading).
            val cb = if (sw.wifiName && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                WifiCallback(withName = true, ConnectivityManager.NetworkCallback.FLAG_INCLUDE_LOCATION_INFO)
            } else {
                WifiCallback(sw.wifiName)
            }
            val request = NetworkRequest.Builder().addTransportType(NetworkCapabilities.TRANSPORT_WIFI).build()
            wifiCallback = cb
            wifiWithName = sw.wifiName
            settle()
            runCatching { getSystemService(ConnectivityManager::class.java).registerNetworkCallback(request, cb) }
                .onFailure {
                    wifiCallback = null
                    graph.log("wifi listener failed: $it", "E")
                }
        }
        // Signal ticks come every few seconds; listen for them only while the bars are reported.
        if (cellCallback != null && (!sw.cellular || (cellCallback?.watch == TelephonyWatch.Watch.SERVICE_STATE_AND_SIGNAL) != sw.cellularSignal)) {
            unwatchCellular()
        }
        if (sw.cellular && cellCallback == null) {
            // Service state changes carry carrier changes; signal changes carry the bars. A callback
            // already queued from a replaced listener (e.g. the old data SIM) is ignored.
            val watch = if (sw.cellularSignal) TelephonyWatch.Watch.SERVICE_STATE_AND_SIGNAL else TelephonyWatch.Watch.SERVICE_STATE
            val cb = TelephonyWatch(watch) { watch, e ->
                if (cellCallback !== watch) return@TelephonyWatch
                if (e is TelephonyWatch.Event.Signal) cellSignal = e.strength
                events.trySend(GatewayEvent.CellularChanged)
            }
            val sub = SubscriptionManager.getDefaultDataSubscriptionId()
            // Set before registering: the first callback may arrive before onSuccess runs.
            cellCallback = cb
            cellSub = sub
            settle()
            runCatching { cb.start(telemetry.dataTelephony(), mainExecutor) }
                .onFailure {
                    cellCallback = null
                    cellSub = null
                    graph.log("cellular listener failed: $it", "E")
                }
        }
        // The generation (5G on an LTE anchor shows only here) and the mobile data switch need READ_PHONE_STATE.
        if (sw.cellularSignal && cellCallback != null && displayCallback == null &&
            telemetry.granted(Manifest.permission.READ_PHONE_STATE)
        ) {
            val cb = TelephonyWatch(TelephonyWatch.Watch.DISPLAY_INFO_AND_MOBILE_DATA) { watch, e ->
                if (displayCallback !== watch) return@TelephonyWatch
                if (e is TelephonyWatch.Event.Display) cellDisplay = e.info
                events.trySend(GatewayEvent.CellularChanged)
            }
            displayCallback = cb
            settle()
            runCatching { cb.start(telemetry.dataTelephony(), mainExecutor) }
                .onFailure {
                    displayCallback = null
                    graph.log("display info listener failed: $it", "E")
                }
        } else if (!sw.cellularSignal) {
            unwatchDisplay()
        }
    }

    /**
     * Follows one Wi-Fi network at a time, so a second one (dual-STA) can't overwrite or clear it.
     * A callback already queued from a replaced listener is ignored.
     */
    private inner class WifiCallback : ConnectivityManager.NetworkCallback {
        private val withName: Boolean

        constructor(withName: Boolean) : super() {
            this.withName = withName
        }

        @TargetApi(Build.VERSION_CODES.S)
        constructor(withName: Boolean, flags: Int) : super(flags) {
            this.withName = withName
        }

        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
            if (wifiCallback !== this) return
            val cur = wifi
            if (cur != null && cur.first != network) return
            wifi = network to telemetry.wifiReading(caps, withName)
            events.trySend(GatewayEvent.RadiosChanged)
        }

        override fun onLost(network: Network) {
            if (wifiCallback !== this || wifi?.first != network) return
            wifi = null
            events.trySend(GatewayEvent.RadiosChanged)
        }
    }

    /** A listener just started: its first readings arrive shortly, so reports wait for them. */
    private fun settle() {
        settleUntil = SystemClock.elapsedRealtime() + RadioReport.SETTLE_MS
    }

    private fun unwatchWifi() {
        wifiCallback?.let { cb -> runCatching { getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(cb) } }
        wifiCallback = null
        wifi = null
    }

    private fun unwatchCellular() {
        cellCallback?.stop()
        cellCallback = null
        cellSub = null
        cellSignal = null
        unwatchDisplay()
    }

    private fun unwatchDisplay() {
        displayCallback?.stop()
        displayCallback = null
        cellDisplay = null
    }

    private fun maybeSendHealthOnNetworkChange() {
        if (GatewayPolicy.reportNetwork(welcomed, reportedNetwork, currentNetwork)) sendHealth()
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
        val state = stateFlow.value
        val content = Notifications.content(state)
        if (!stopping && content != notified) {
            runCatching {
                getSystemService(NotificationManager::class.java).notify(Notifications.ID_GATEWAY, Notifications.gateway(this, state))
                notified = content
            }
        }
    }

    companion object {
        /** Intent.ACTION_SIM_STATE_CHANGED, a system broadcast without a public constant. */
        private const val ACTION_SIM_STATE_CHANGED = "android.intent.action.SIM_STATE_CHANGED"


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

        /** A Network switch changed on the phone (spec 015). */
        fun onRadioSettings() {
            eventsOut?.trySend(GatewayEvent.RadioSettings)
        }

        /** If the socket is open, send a health frame carrying [token]. */
        fun onFcmToken(token: String) {
            eventsOut?.trySend(GatewayEvent.FcmToken(token))
        }
    }
}
