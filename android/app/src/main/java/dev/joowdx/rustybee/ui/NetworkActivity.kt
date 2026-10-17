package dev.joowdx.rustybee.ui

import android.Manifest
import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import android.provider.Settings
import android.view.View
import android.widget.Switch
import android.widget.TextView
import dev.joowdx.rustybee.gateway.GatewayService
import dev.joowdx.rustybee.gateway.Graph
import dev.joowdx.rustybee.gateway.RadioSwitches
import dev.joowdx.rustybee.gateway.Telemetry

/**
 * Opt-in network details (spec 015): per radio, its name and signal strength. Everything is off by
 * default; the connection type is always reported. The Wi-Fi name needs location "all the time".
 */
class NetworkActivity : Activity() {
    private val prefs by lazy { Graph.of(this).prefs }
    private val telemetry by lazy { Telemetry(this) }

    private lateinit var cellularName: Switch
    private lateinit var cellularSignal: Switch
    private lateinit var wifiName: Switch
    private lateinit var wifiSignal: Switch
    private lateinit var wifiHint: TextView
    private lateinit var grant: View
    /** The location disclosure was accepted on this screen, so the runtime prompts may follow. */
    private var disclosed = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val content = vertical()
        content.add(text("Network", 24f, bold = true))
        content.add(
            text("Choose what this phone reports about its connection. The connection type is always reported.").apply {
                setPadding(0, dp(8), 0, 0)
            },
        )
        val sw = prefs.radioSwitches

        content.add(header("Cellular"))
        cellularName = toggle("Carrier name", sw.cellularName)
        cellularSignal = toggle("Signal strength", sw.cellularSignal)
        val cellularHint = text("Bars, and 2G to 5G while mobile data is on.", 14f).apply { setPadding(0, dp(4), 0, 0) }
        content.add(cellularName).add(cellularSignal).add(cellularHint)

        content.add(header("Wi-Fi"))
        wifiName = toggle("Network name", sw.wifiName) { on -> if (on) askLocation() }
        wifiHint = text("", 14f).apply {
            setTextColor(0xFFB45309.toInt())
            setPadding(0, dp(4), 0, 0)
        }
        grant = button("Grant") { askLocation() }
        wifiSignal = toggle("Signal strength", sw.wifiSignal)
        content.add(wifiName).add(wifiHint).add(grant).add(wifiSignal)
        scrollingRoot(content)
    }

    override fun onResume() {
        super.onResume()
        // A permission granted in Settings shows up here; the service re-checks what it can read.
        refreshHint()
        GatewayService.onRadioSettings()
    }

    private fun toggle(label: String, on: Boolean, onChange: (Boolean) -> Unit = {}): Switch = Switch(this).apply {
        text = label
        textSize = 16f
        isChecked = on
        setPadding(0, dp(12), 0, 0)
        setOnCheckedChangeListener { _, checked ->
            save()
            onChange(checked)
        }
    }

    private fun save() {
        prefs.radioSwitches = RadioSwitches(
            cellularName = cellularName.isChecked,
            cellularSignal = cellularSignal.isChecked,
            wifiName = wifiName.isChecked,
            wifiSignal = wifiSignal.isChecked,
        )
        refreshHint()
        GatewayService.onRadioSettings()
    }

    private fun refreshHint() {
        val blocker = if (wifiName.isChecked) telemetry.wifiNameBlocker() else null
        wifiHint.text = blocker?.let { "$it Until then the name is left out." }.orEmpty()
        wifiHint.visibility = if (blocker == null) View.GONE else View.VISIBLE
        grant.visibility = wifiHint.visibility
    }

    /**
     * One step at a time: precise location, then "Allow all the time", then the Location switch. A
     * disclosure explains the background use before any permission prompt, and can be declined.
     */
    private fun askLocation() {
        val prompts = !granted(Manifest.permission.ACCESS_FINE_LOCATION) ||
            !granted(Manifest.permission.ACCESS_BACKGROUND_LOCATION)
        if (prompts && !disclosed) {
            AlertDialog.Builder(this)
                .setTitle("Location for the Wi-Fi name")
                .setMessage(
                    "Android reveals the Wi-Fi network name only to apps with location access. This app uses it " +
                        "only to send the Wi-Fi name to your server, also while the app is closed or not in use, " +
                        "so it needs location \"Allow all the time\". It never reads or sends your location.",
                )
                .setPositiveButton("Continue") { _, _ ->
                    disclosed = true
                    askLocation()
                }
                .setNegativeButton("Not now", null)
                .show()
            return
        }
        when {
            !granted(Manifest.permission.ACCESS_FINE_LOCATION) -> requestPermissions(
                arrayOf(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION),
                REQ_FOREGROUND,
            )
            !granted(Manifest.permission.ACCESS_BACKGROUND_LOCATION) ->
                requestPermissions(arrayOf(Manifest.permission.ACCESS_BACKGROUND_LOCATION), REQ_BACKGROUND)
            telemetry.wifiNameBlocker() != null -> startActivity(Intent(Settings.ACTION_LOCATION_SOURCE_SETTINGS))
        }
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == REQ_FOREGROUND && granted(Manifest.permission.ACCESS_FINE_LOCATION)) askLocation()
        refreshHint()
        GatewayService.onRadioSettings()
    }

    private fun granted(p: String) = checkSelfPermission(p) == PackageManager.PERMISSION_GRANTED

    private companion object {
        const val REQ_FOREGROUND = 1
        const val REQ_BACKGROUND = 2
    }
}
