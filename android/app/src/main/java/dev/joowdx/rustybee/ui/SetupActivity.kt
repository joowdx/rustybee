package dev.joowdx.rustybee.ui

import android.annotation.SuppressLint
import android.annotation.TargetApi
import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.text.Editable
import android.text.InputType
import android.text.TextWatcher
import android.view.View
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.TextView
import android.window.OnBackInvokedCallback
import android.window.OnBackInvokedDispatcher
import dev.joowdx.rustybee.BuildConfig
import dev.joowdx.rustybee.gateway.Cleartext
import dev.joowdx.rustybee.gateway.GatewayService
import dev.joowdx.rustybee.gateway.Graph
import dev.joowdx.rustybee.gateway.Telemetry
import dev.joowdx.rustybee.gateway.Watchdog
import dev.joowdx.rustybee.protocol.ApiErrorBody
import dev.joowdx.rustybee.protocol.PairRequest
import dev.joowdx.rustybee.protocol.PairResponse
import dev.joowdx.rustybee.protocol.ProtocolJson
import dev.joowdx.rustybee.push.Push
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import java.util.concurrent.TimeUnit

/** Pairing via the in-app QR scanner, the `rustybee://pair?u=…&c=…` deep link, or a pasted link (spec 013). */
class SetupActivity : Activity() {
    private val scope: CoroutineScope = MainScope()
    private var pairing: Job? = null

    private lateinit var link: EditText
    private lateinit var insecure: CheckBox
    private lateinit var error: TextView
    private lateinit var pairButton: Button

    // Back cancels a running pairing. Created only on API 33+: the callback type is missing on Android 11.
    private var cancelPairing: Any? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val content = vertical()
        content.add(text("RustyBee", 28f, bold = true))
        content.add(
            text(
                "Scan the pairing QR code on the server's Devices page, or paste the pairing link below.",
            ).apply { setPadding(0, dp(8), 0, dp(16)) },
        )
        content.add(button("Scan QR code") { startActivityForResult(Intent(this, ScanActivity::class.java), REQ_SCAN) })
        content.add(header("Or paste the link"))
        link = EditText(this).apply {
            hint = "rustybee://pair?u=…&c=…"
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI
            isSingleLine = true
        }
        insecure = CheckBox(this).apply {
            text = "Allow insecure http (LAN)"
            visibility = View.GONE
        }
        error = text("").apply {
            setTextColor(0xFFDC2626.toInt())
            visibility = View.GONE
            setPadding(0, dp(8), 0, dp(8))
        }
        pairButton = button("Pair") { pair() }
        content.add(link).add(insecure).add(error).add(pairButton)
        scrollingRoot(content)

        link.addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) = Unit
            override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) = Unit
            override fun afterTextChanged(s: Editable?) = updateInsecureVisibility()
        })
        // Why the pairing was dropped without the user asking (revoked, token refused, Keystore key lost).
        showError(Graph.of(this).prefs.setupNotice)
        handle(intent)
    }

    @Deprecated("Platform Activity result API; the app avoids AndroidX activity.")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != REQ_SCAN) return
        when (resultCode) {
            RESULT_OK -> handle(data)
            ScanActivity.RESULT_DENIED ->
                showError("Camera access was refused. Allow it in the app's settings, or paste the link instead.")
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    private fun handle(intent: Intent?) {
        // Incoming links only fill the field; pairing needs the Pair button and the confirm dialog (AC-013-07).
        val raw = PairConfirm.linkFromIntent(intent?.data?.toString()) ?: return
        link.setText(raw)
        updateInsecureVisibility()
    }

    /** Returns (base URL, code) or null. */
    private fun parse(raw: String): Pair<String, String>? {
        val uri = runCatching { Uri.parse(raw.trim()) }.getOrNull() ?: return null
        if (uri.scheme != "rustybee" || uri.host != "pair") return null
        val u = uri.getQueryParameter("u")?.trimEnd('/') ?: return null
        val c = uri.getQueryParameter("c") ?: return null
        if (!u.startsWith("https://") && !u.startsWith("http://")) return null
        return u to c
    }

    private fun updateInsecureVisibility() {
        val http = parse(link.text.toString())?.first?.startsWith("http://") == true
        insecure.visibility = if (http) View.VISIBLE else View.GONE
    }

    private fun showError(message: String?) {
        error.text = message ?: ""
        error.visibility = if (message == null) View.GONE else View.VISIBLE
    }

    private fun pair() {
        if (pairing?.isActive == true) return
        val parsed = parse(link.text.toString())
            ?: return showError("That is not a valid rustybee://pair link.")
        val (base, code) = parsed
        val allowInsecure = insecure.isChecked
        if (!Cleartext.allowed(base)) return showError(Cleartext.REFUSED)
        if (base.startsWith("http://") && !allowInsecure && !BuildConfig.DEBUG) {
            return showError("This server uses plain http. Tick \"Allow insecure http (LAN)\" to continue.")
        }
        showError(null)
        val host = Uri.parse(base).host ?: base
        val paired = Graph.of(this).prefs.paired
        AlertDialog.Builder(this)
            .setTitle(PairConfirm.title(paired))
            .setMessage(PairConfirm.message(host, paired))
            .setPositiveButton(if (paired) "Replace and pair" else "Pair") { _, _ -> startPairing(base, code, allowInsecure) }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun startPairing(base: String, code: String, allowInsecure: Boolean) {
        if (pairing?.isActive == true) return
        pairButton.isEnabled = false
        pairButton.text = "Pairing…"
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) interceptBack(true)
        pairing = scope.launch {
            try {
                val result = withContext(Dispatchers.IO) { request(base, code) }
                if (!Cleartext.allowed(result.wsUrl)) {
                    showError("The server returned a plain ws:// URL for a public host. ${Cleartext.REFUSED}")
                    return@launch
                }
                if (result.wsUrl.startsWith("ws://") && !allowInsecure && !BuildConfig.DEBUG) {
                    showError("The server returned an insecure ws:// URL. Tick \"Allow insecure http (LAN)\" to continue.")
                    return@launch
                }
                val graph = Graph.of(this@SetupActivity)
                // Re-pairing: stop the gateway first so it cannot stay connected to the old server, then
                // drop the old server's settings (forwarding "off", config, last connected); the FCM token stays.
                Watchdog.cancel(this@SetupActivity)
                GatewayService.stop(this@SetupActivity)
                graph.prefs.clearPairing()
                graph.db.wipe()
                graph.prefs.apply {
                    serverUrl = base
                    wsUrl = result.wsUrl
                    deviceId = result.deviceId
                    token = result.deviceToken
                    this.allowInsecure = allowInsecure
                    gatewayEnabled = true
                }
                graph.log("paired with ${Uri.parse(base).host}")
                startActivity(
                    Intent(this@SetupActivity, MainActivity::class.java)
                        .putExtra(MainActivity.EXTRA_FIRST_RUN, true)
                        .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TASK or Intent.FLAG_ACTIVITY_NEW_TASK),
                )
                finish()
            } catch (e: PairingException) {
                showError(e.message)
            } catch (e: java.io.IOException) {
                showError("Could not reach the server: ${e.message ?: e.javaClass.simpleName}")
            } finally {
                pairButton.isEnabled = true
                pairButton.text = "Pair"
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) interceptBack(false)
            }
        }
    }

    @TargetApi(Build.VERSION_CODES.TIRAMISU)
    private fun interceptBack(on: Boolean) {
        val cb = (cancelPairing as? OnBackInvokedCallback) ?: OnBackInvokedCallback { pairing?.cancel() }.also { cancelPairing = it }
        if (on) {
            onBackInvokedDispatcher.registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_DEFAULT, cb)
        } else {
            onBackInvokedDispatcher.unregisterOnBackInvokedCallback(cb)
        }
    }

    /** Below API 33 back arrives here (spec 011); from 33 [interceptBack] handles it instead. */
    @SuppressLint("GestureBackNavigation")
    @Deprecated("Replaced by OnBackInvokedDispatcher from API 33")
    override fun onBackPressed() {
        val p = pairing
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU && p?.isActive == true) {
            p.cancel()
        } else {
            @Suppress("DEPRECATION")
            super.onBackPressed()
        }
    }

    private companion object {
        const val REQ_SCAN = 1
    }

    private class PairingException(message: String) : Exception(message)

    private suspend fun request(base: String, code: String): PairResponse {
        val telemetry = Telemetry(this)
        val body = PairRequest(
            code = code,
            name = telemetry.deviceName(),
            manufacturer = Build.MANUFACTURER,
            model = Build.MODEL,
            osVersion = Build.VERSION.RELEASE,
            appVersion = BuildConfig.VERSION_NAME,
            appBuild = BuildConfig.VERSION_CODE,
            fcmToken = Push.refresh(this),
        )
        val client = OkHttpClient.Builder().callTimeout(30, TimeUnit.SECONDS).build()
        val req = Request.Builder()
            .url("$base/api/device/pair")
            .post(ProtocolJson.encodeToString(PairRequest.serializer(), body).toRequestBody("application/json".toMediaType()))
            .build()
        client.newCall(req).execute().use { resp ->
            val text = resp.body.string()
            if (resp.isSuccessful) {
                return runCatching { ProtocolJson.decodeFromString(PairResponse.serializer(), text) }
                    .getOrElse { throw PairingException("The server sent an unexpected response.") }
            }
            val apiCode = runCatching { ProtocolJson.decodeFromString(ApiErrorBody.serializer(), text).error.code }.getOrNull()
            throw PairingException(
                when {
                    apiCode == "pairing_invalid" || resp.code == 401 ->
                        "This pairing code is invalid, expired or already used. Create a new one in the web UI."
                    resp.code == 400 -> "The server rejected the request (bad_request)."
                    else -> "Pairing failed: HTTP ${resp.code}"
                },
            )
        }
    }
}
