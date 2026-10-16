package dev.joowdx.rustybee.ui

import android.app.Activity
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
import dev.joowdx.rustybee.gateway.Graph
import dev.joowdx.rustybee.gateway.Telemetry
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

    private val cancelPairing = OnBackInvokedCallback { pairing?.cancel() }

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
        val data = intent?.data ?: return
        if (data.scheme == "rustybee") {
            link.setText(data.toString())
            if (parse(data.toString())?.first?.startsWith("https://") == true) pair()
        }
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
        if (base.startsWith("http://") && !allowInsecure && !BuildConfig.DEBUG) {
            return showError("This server uses plain http. Tick \"Allow insecure http (LAN)\" to continue.")
        }
        showError(null)
        pairButton.isEnabled = false
        pairButton.text = "Pairing…"
        onBackInvokedDispatcher.registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_DEFAULT, cancelPairing)
        pairing = scope.launch {
            try {
                val result = withContext(Dispatchers.IO) { request(base, code) }
                if (result.wsUrl.startsWith("ws://") && !allowInsecure && !BuildConfig.DEBUG) {
                    showError("The server returned an insecure ws:// URL. Tick \"Allow insecure http (LAN)\" to continue.")
                    return@launch
                }
                val graph = Graph.of(this@SetupActivity)
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
                onBackInvokedDispatcher.unregisterOnBackInvokedCallback(cancelPairing)
            }
        }
    }

    private companion object {
        const val REQ_SCAN = 1
    }

    private class PairingException(message: String) : Exception(message)

    private fun request(base: String, code: String): PairResponse {
        val telemetry = Telemetry(this)
        val body = PairRequest(
            code = code,
            name = telemetry.deviceName(),
            manufacturer = Build.MANUFACTURER,
            model = Build.MODEL,
            osVersion = Build.VERSION.RELEASE,
            appVersion = BuildConfig.VERSION_NAME,
            appBuild = BuildConfig.VERSION_CODE,
            fcmToken = Push.token(this),
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
