package dev.joowdx.rustybee.ui

import android.app.Activity
import android.content.ComponentName
import android.content.Intent
import android.content.ServiceConnection
import android.os.Bundle
import android.os.IBinder
import android.text.InputType
import android.widget.AdapterView
import android.widget.ArrayAdapter
import android.widget.CheckBox
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.Spinner
import android.widget.Switch
import android.widget.TextView
import android.view.View
import android.view.ViewGroup
import dev.joowdx.rustybee.gateway.ConnectionState
import dev.joowdx.rustybee.gateway.Forwarding
import dev.joowdx.rustybee.gateway.ForwardingSave
import dev.joowdx.rustybee.gateway.GatewayService
import dev.joowdx.rustybee.gateway.GatewayState
import dev.joowdx.rustybee.gateway.Graph
import dev.joowdx.rustybee.protocol.FilterRule
import dev.joowdx.rustybee.protocol.Filters
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

/**
 * Turn forwarding on or off and edit the inbound SMS filter (spec 013). Saving needs a live
 * connection, except turning forwarding off, which applies on the phone at once. An edit made
 * against an older revision is refused and reloaded.
 */
class ForwardingActivity : Activity() {
    private val scope = MainScope()
    private var collect: Job? = null
    private val graph by lazy { Graph.of(this) }

    private lateinit var forward: Switch
    private lateinit var filter: LinearLayout
    private lateinit var offNote: TextView
    private lateinit var mode: Spinner
    private lateinit var rows: LinearLayout
    private lateinit var warning: TextView
    private lateinit var status: TextView
    private lateinit var save: View

    private var binder: GatewayService.LocalBinder? = null
    private var bound = false
    /** Revision the editor was loaded from; sent as `base_rev`. */
    private var baseRev = 0L
    private var state = GatewayState()
    /** Only report save results for saves made from this screen. */
    private var submitted = false

    private val connectionCallback = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName, service: IBinder) {
            val b = service as GatewayService.LocalBinder
            binder = b
            collect?.cancel()
            collect = scope.launch { b.state.collect { render(it) } }
        }

        override fun onServiceDisconnected(name: ComponentName) {
            binder = null
            collect?.cancel()
            render(GatewayState())
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val content = vertical()
        content.add(text("Forwarding", 24f, bold = true))
        content.add(text("Choose whether received texts are forwarded to the server, and which.").apply { setPadding(0, dp(8), 0, 0) })

        forward = Switch(this).apply {
            text = "Forward received SMS"
            textSize = 16f
            setPadding(0, dp(16), 0, 0)
            setOnCheckedChangeListener { _, _ -> refreshHints(); render(state) }
        }
        content.add(forward)
        offNote = text("Nothing is forwarded while this is off. Only this phone can turn it back on.", 14f).apply {
            setPadding(0, dp(4), 0, 0)
        }
        content.add(offNote)

        filter = vertical()
        content.add(filter)
        filter.add(header("Filter"))
        mode = Spinner(this).apply {
            adapter = spinnerAdapter(MODES)
            onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
                override fun onItemSelected(p: AdapterView<*>?, v: View?, pos: Int, id: Long) = refreshHints()
                override fun onNothingSelected(p: AdapterView<*>?) = Unit
            }
        }
        filter.add(mode)
        warning = text("", 14f).apply {
            setTextColor(0xFFB45309.toInt())
            setPadding(0, dp(8), 0, 0)
        }
        filter.add(warning)

        filter.add(header("Rules"))
        rows = vertical()
        filter.add(rows)
        filter.add(button("Add rule") { addRow(FilterRule("sender", "contains", "")); refreshHints() })

        content.add(header("Save"))
        status = text("")
        content.add(status)
        save = button("Save") { submit() }
        content.add(horizontal().add(save).add(button("Reload") { load() }))
        scrollingRoot(content)
        load()
    }

    override fun onStart() {
        super.onStart()
        bound = bindService(Intent(this, GatewayService::class.java), connectionCallback, 0)
        if (!bound) render(GatewayState())
    }

    override fun onStop() {
        collect?.cancel()
        if (bound) unbindService(connectionCallback)
        bound = false
        binder = null
        super.onStop()
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    /** Fill the editor from the config the server last sent and this phone's own "off". */
    private fun load() {
        val config = graph.prefs.config
        baseRev = config.filtersRev
        forward.isChecked = Forwarding.on(config, graph.prefs.forwardingOffHere)
        mode.setSelection(MODES.indexOfFirst { it.first == config.filters.mode }.coerceAtLeast(0))
        rows.removeAllViews()
        config.filters.rules.forEach(::addRow)
        refreshHints()
    }

    private fun addRow(rule: FilterRule) {
        val field = spinner(FIELDS, rule.field)
        val op = spinner(OPS, rule.op)
        val value = EditText(this).apply {
            hint = "Text to match"
            inputType = InputType.TYPE_CLASS_TEXT
            setText(rule.value)
        }
        val ignoreCase = CheckBox(this).apply {
            text = "Ignore case"
            isChecked = rule.ignoreCase
        }
        val row = vertical().apply { setPadding(0, dp(8), 0, dp(8)) }
        row.add(horizontal().add(field, 1f).add(op, 1f))
        row.add(value)
        row.add(horizontal().add(ignoreCase, 1f).add(button("Remove") { rows.removeView(row); refreshHints() }))
        row.tag = RuleViews(field, op, value, ignoreCase)
        rows.add(row)
    }

    private fun spinner(options: List<Pair<String, String>>, selected: String) = Spinner(this).apply {
        adapter = spinnerAdapter(options)
        setSelection(options.indexOfFirst { it.first == selected }.coerceAtLeast(0))
    }

    /** Closed spinners use the plain item layout; the dropdown row layout measures to zero height there. */
    private fun spinnerAdapter(options: List<Pair<String, String>>) =
        ArrayAdapter(this, android.R.layout.simple_spinner_item, options.map { it.second }).apply {
            setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item)
        }

    private fun current(): Filters {
        val rules = (0 until rows.childCount).map { i ->
            val v = rows.getChildAt(i).tag as RuleViews
            FilterRule(
                field = FIELDS[v.field.selectedItemPosition].first,
                op = OPS[v.op.selectedItemPosition].first,
                value = v.value.text.toString().trim(),
                ignoreCase = v.ignoreCase.isChecked,
            )
        }
        return Filters(MODES[mode.selectedItemPosition].first, rules)
    }

    private fun refreshHints() {
        offNote.visibility = if (forward.isChecked) View.GONE else View.VISIBLE
        setEnabledDeep(filter, forward.isChecked)
        filter.alpha = if (forward.isChecked) 1f else 0.5f
        val m = MODES[mode.selectedItemPosition].first
        warning.text = when {
            m == "allow" && rows.childCount == 0 -> "With no rules, nothing is forwarded."
            m == "off" && rows.childCount > 0 -> "Rules are ignored while the filter is off."
            else -> ""
        }
        warning.visibility = if (warning.text.isEmpty()) View.GONE else View.VISIBLE
    }

    private fun setEnabledDeep(v: View, enabled: Boolean) {
        v.isEnabled = enabled
        if (v is ViewGroup) (0 until v.childCount).forEach { setEnabledDeep(v.getChildAt(it), enabled) }
    }

    private fun submit() {
        // The filter is locked while forwarding is off, so turning it off keeps the stored one.
        val filters = if (forward.isChecked) current() else graph.prefs.config.filters
        if (filters.rules.any { it.value.isEmpty() }) {
            status.text = "Every rule needs text to match."
            return
        }
        if (filters.rules.size > MAX_RULES) {
            status.text = "At most $MAX_RULES rules."
            return
        }
        val b = binder ?: return render(state)
        submitted = true
        b.saveForwarding(forward.isChecked, filters, baseRev)
    }

    private fun render(s: GatewayState) {
        val prev = state
        state = s
        val online = binder != null && s.connection == ConnectionState.CONNECTED
        // Turning forwarding off works offline (spec 002).
        save.isEnabled = (online || !forward.isChecked) && binder != null && s.forwardingSave != ForwardingSave.Saving
        // The server's answer to our save arrives with (or right after) its new config.
        if (submitted && s.filtersRev != prev.filtersRev) {
            if (s.forwardingSave == ForwardingSave.Saved) baseRev = s.filtersRev
            if (s.forwardingSave == ForwardingSave.Conflict) load()
        }
        val f = s.forwardingSave
        status.text = when {
            submitted && f == ForwardingSave.SavedOnPhone -> "Off on this phone. The server is told when the phone reconnects."
            !online && forward.isChecked -> "Connect to the server to save. The phone keeps using the last settings it received."
            !online -> "Not connected. Saving turns forwarding off on this phone now."
            !submitted -> ""
            f == ForwardingSave.Saving -> "Saving…"
            f == ForwardingSave.Saved -> "Saved."
            f == ForwardingSave.Conflict -> "Changed on the web, reloaded. Make your edit again."
            f is ForwardingSave.Failed -> "Not saved: ${f.message}"
            else -> ""
        }
    }

    private class RuleViews(val field: Spinner, val op: Spinner, val value: EditText, val ignoreCase: CheckBox)

    private companion object {
        const val MAX_RULES = 100
        val MODES = listOf(
            "off" to "Off: forward every text",
            "allow" to "Forward only matching texts",
            "block" to "Forward all except matching",
        )
        val FIELDS = listOf("sender" to "Sender", "body" to "Message", "any" to "Sender or message")
        val OPS = listOf("contains" to "contains", "equals" to "equals", "starts_with" to "starts with", "ends_with" to "ends with")
    }
}
