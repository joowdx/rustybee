package dev.joowdx.rustybee.ui

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.app.AlertDialog
import android.content.ComponentName
import android.content.Intent
import android.content.ServiceConnection
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.os.IBinder
import android.provider.Settings
import android.telephony.SubscriptionManager
import android.view.View
import android.widget.LinearLayout
import android.widget.Switch
import android.widget.TextView
import dev.joowdx.rustybee.gateway.ConnectionState
import dev.joowdx.rustybee.gateway.GatewayService
import dev.joowdx.rustybee.gateway.GatewayState
import dev.joowdx.rustybee.gateway.Graph
import dev.joowdx.rustybee.gateway.Telemetry
import dev.joowdx.rustybee.gateway.Watchdog
import dev.joowdx.rustybee.push.Push
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import java.text.DateFormat
import java.util.Date

/** Status, health checklist, SIMs and device actions on one scrolling screen (spec 013). */
class MainActivity : Activity() {
    private val scope = MainScope()
    private var collect: Job? = null
    private val graph by lazy { Graph.of(this) }
    private val telemetry by lazy { Telemetry(this) }

    private lateinit var dot: TextView
    private lateinit var stateText: TextView
    private lateinit var details: TextView
    private lateinit var toggle: Switch
    private lateinit var checklist: LinearLayout
    private lateinit var sims: LinearLayout

    private var bound = false
    private var lastState = GatewayState()

    private val connectionCallback = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName, service: IBinder) {
            val flow = (service as GatewayService.LocalBinder).state
            collect?.cancel()
            collect = scope.launch { flow.collect { render(it) } }
        }

        override fun onServiceDisconnected(name: ComponentName) {
            collect?.cancel()
            render(GatewayState())
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (!graph.prefs.paired) {
            startActivity(Intent(this, SetupActivity::class.java))
            finish()
            return
        }
        buildUi()
        if (savedInstanceState == null && intent.getBooleanExtra(EXTRA_FIRST_RUN, false)) enableGateway()
    }

    override fun onStart() {
        super.onStart()
        if (!::toggle.isInitialized) return
        bound = bindService(Intent(this, GatewayService::class.java), connectionCallback, 0)
    }

    override fun onResume() {
        super.onResume()
        if (!::toggle.isInitialized) return
        if (!graph.prefs.paired) return openSetup()
        refreshStatic()
        if (graph.prefs.gatewayEnabled && GatewayService.running) GatewayService.start(this) // permission re-check
    }

    override fun onStop() {
        collect?.cancel()
        if (bound) unbindService(connectionCallback)
        bound = false
        super.onStop()
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    // --- UI

    private fun buildUi() {
        val content = vertical()
        content.add(text("RustyBee", 28f, bold = true))

        content.add(header("Status"))
        dot = text("●", 18f)
        stateText = text("", 17f, bold = true).apply { setPadding(dp(8), 0, 0, 0) }
        toggle = Switch(this).apply {
            text = "Gateway"
            setOnCheckedChangeListener { _, on -> if (on) enableGateway() else disableGateway() }
        }
        content.add(horizontal().add(dot).add(stateText, 1f).add(toggle))
        details = text("").apply { setPadding(0, dp(8), 0, 0) }
        content.add(details)

        content.add(header("Health"))
        checklist = vertical()
        content.add(checklist)

        content.add(header("SIMs"))
        sims = vertical()
        content.add(sims)

        content.add(header("Device"))
        content.add(
            horizontal()
                .add(button("Filter") { startActivity(Intent(this, FiltersActivity::class.java)) })
                .add(button("Logs") { startActivity(Intent(this, LogsActivity::class.java)) })
                .add(button("Unpair") { confirmUnpair() }),
        )
        scrollingRoot(content)
    }

    private fun refreshStatic() {
        toggle.setOnCheckedChangeListener(null)
        toggle.isChecked = graph.prefs.gatewayEnabled
        toggle.setOnCheckedChangeListener { _, on -> if (on) enableGateway() else disableGateway() }
        render(lastState.takeIf { GatewayService.running } ?: GatewayState())
        refreshChecklist()
        refreshSims()
    }

    private fun render(state: GatewayState) {
        lastState = state
        val (label, color) = when {
            !graph.prefs.gatewayEnabled -> "Stopped" to 0xFF9CA3AF
            state.connection == ConnectionState.CONNECTED -> "Connected" to 0xFF16A34A
            state.connection == ConnectionState.CONNECTING -> "Reconnecting…" to 0xFFF59E0B
            else -> "Stopped" to 0xFF9CA3AF
        }
        dot.setTextColor(color.toInt())
        stateText.text = label
        val prefs = graph.prefs
        val last = prefs.lastConnectedAt.takeIf { it > 0 }
            ?.let { DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT).format(Date(it)) } ?: "never"
        details.text = buildString {
            appendLine("Server: ${prefs.serverUrl?.let { Uri.parse(it).host } ?: "-"}")
            appendLine("Device: ${prefs.config.name.ifEmpty { "-" }}")
            appendLine("Last connected: $last")
            appendLine("Outbox: ${graph.uplink.size()}")
            append("Today: ${prefs.today("sends")} sent, ${prefs.today("receives")} received")
            state.lastError?.let { append("\nLast error: $it") }
        }
    }

    private fun checkRow(label: String, ok: Boolean, action: String?, onAction: () -> Unit): View {
        val row = horizontal()
        row.add(text(if (ok) "✓" else "✗", 17f, bold = true).apply {
            setTextColor((if (ok) 0xFF16A34A else 0xFFDC2626).toInt())
            setPadding(0, 0, dp(12), 0)
        })
        row.add(text(label), 1f)
        if (action != null) row.add(button(action, onAction))
        return row
    }

    private fun granted(p: String) = checkSelfPermission(p) == PackageManager.PERMISSION_GRANTED

    private fun refreshChecklist() {
        checklist.removeAllViews()
        val sms = SMS_PERMISSIONS.all(::granted)
        checklist.add(checkRow("SMS permissions (send, receive, phone state)", sms, if (sms) null else "Grant") {
            requestRuntimePermissions()
        })
        val notif = telemetry.permissions().notifications
        checklist.add(checkRow("Notifications", notif, if (notif) null else "Allow") {
            if (!granted(Manifest.permission.POST_NOTIFICATIONS)) {
                requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), REQ_PERMS)
            } else {
                startActivity(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, packageName))
            }
        })
        val battery = telemetry.batteryUnrestricted()
        checklist.add(checkRow("Battery unrestricted", battery, if (battery) null else "Allow") { requestBatteryExemption() })
        if (!battery) {
            checklist.add(
                text("Without this, Android may stop the gateway in the background and delay SMS forwarding.", 13f)
                    .apply { alpha = 0.7f },
            )
        }
        if (Push.SUPPORTED) {
            val token = Push.token(this) != null
            checklist.add(checkRow("Push wake (FCM token)", token, if (token) null else "Retry") {
                Push.refresh(this); refreshChecklist()
            })
        }
        checklist.add(
            text(
                "If SMS permissions are greyed out: open App info, tap ⋮, choose \"Allow restricted settings\", then grant them.",
                13f,
            ).apply { alpha = 0.7f; setPadding(0, dp(8), 0, 0) },
        )
        checklist.add(button("Open app info") {
            startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", packageName, null)))
        })
    }

    @SuppressLint("MissingPermission")
    private fun refreshSims() {
        sims.removeAllViews()
        if (!granted(Manifest.permission.READ_PHONE_STATE)) {
            sims.add(text("Grant phone state permission to list SIMs."))
            return
        }
        val list = getSystemService(SubscriptionManager::class.java)?.activeSubscriptionInfoList.orEmpty()
        if (list.isEmpty()) sims.add(text("No active SIM"))
        list.forEach {
            sims.add(text("Slot ${it.simSlotIndex + 1}: ${it.carrierName ?: it.displayName} (id ${it.subscriptionId})"))
        }
        sims.add(text("The preferred SIM is set in the web UI.", 13f).apply { alpha = 0.7f })
    }

    // --- actions

    private fun enableGateway() {
        graph.prefs.gatewayEnabled = true
        toggle.setOnCheckedChangeListener(null)
        toggle.isChecked = true
        toggle.setOnCheckedChangeListener { _, on -> if (on) enableGateway() else disableGateway() }
        if (RUNTIME_PERMISSIONS.all(::granted)) {
            startGateway()
        } else {
            AlertDialog.Builder(this)
                .setTitle("Permissions")
                .setMessage(
                    "RustyBee needs to send and receive SMS, read the SIM list, and show its status notification " +
                        "to work as your SMS gateway.",
                )
                .setPositiveButton("Continue") { _, _ -> requestRuntimePermissions() }
                .setOnCancelListener { startGateway() }
                .show()
        }
    }

    private fun requestRuntimePermissions() {
        requestPermissions(RUNTIME_PERMISSIONS, REQ_PERMS)
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != REQ_PERMS) return
        if (graph.prefs.gatewayEnabled) startGateway()
        refreshChecklist()
        refreshSims()
    }

    private fun startGateway() {
        Watchdog.schedule(this)
        GatewayService.start(this)
        if (!bound) bound = bindService(Intent(this, GatewayService::class.java), connectionCallback, 0)
        refreshChecklist()
    }

    private fun disableGateway() {
        graph.prefs.gatewayEnabled = false
        Watchdog.cancel(this)
        GatewayService.stop(this)
        render(GatewayState())
    }

    @SuppressLint("BatteryLife")
    private fun requestBatteryExemption() {
        AlertDialog.Builder(this)
            .setTitle("Battery")
            .setMessage(
                "Letting RustyBee run unrestricted keeps the gateway connection alive, so messages are sent and " +
                    "received within seconds. It stays idle unless it is sending, receiving or reconnecting.",
            )
            .setPositiveButton("Continue") { _, _ ->
                startActivity(
                    Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName")),
                )
            }
            .setNegativeButton("Not now", null)
            .show()
    }

    private fun confirmUnpair() {
        AlertDialog.Builder(this)
            .setTitle("Unpair this phone?")
            .setMessage("This removes the server credentials and all local data. You will need a new pairing code.")
            .setPositiveButton("Unpair") { _, _ -> unpair() }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun unpair() {
        Watchdog.cancel(this)
        GatewayService.stop(this)
        graph.prefs.clearPairing()
        graph.db.wipe()
        openSetup()
    }

    private fun openSetup() {
        startActivity(Intent(this, SetupActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_CLEAR_TASK or Intent.FLAG_ACTIVITY_NEW_TASK))
        finish()
    }

    companion object {
        const val EXTRA_FIRST_RUN = "first_run"
        private const val REQ_PERMS = 1
        private val SMS_PERMISSIONS = arrayOf(
            Manifest.permission.SEND_SMS,
            Manifest.permission.RECEIVE_SMS,
            Manifest.permission.READ_PHONE_STATE,
        )
        private val RUNTIME_PERMISSIONS = SMS_PERMISSIONS + Manifest.permission.POST_NOTIFICATIONS
    }
}
