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
import android.widget.TextView
import android.view.View
import dev.joowdx.rustybee.gateway.ConnectionState
import dev.joowdx.rustybee.gateway.FiltersSave
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
 * Edit this device's inbound SMS filter (spec 013). The server owns the filter: saving
 * needs a live connection, and an edit made against an older revision is refused and reloaded.
 */
class FiltersActivity : Activity() {
    private val scope = MainScope()
    private var collect: Job? = null
    private val graph by lazy { Graph.of(this) }

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
        content.add(text("SMS filter", 24f, bold = true))
        content.add(text("Choose which received texts are forwarded to the server.").apply { setPadding(0, dp(8), 0, 0) })

        content.add(header("Mode"))
        mode = Spinner(this).apply {
            adapter = ArrayAdapter(context, android.R.layout.simple_spinner_dropdown_item, MODES.map { it.second })
            onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
                override fun onItemSelected(p: AdapterView<*>?, v: View?, pos: Int, id: Long) = refreshHints()
                override fun onNothingSelected(p: AdapterView<*>?) = Unit
            }
        }
        content.add(mode)
        warning = text("", 14f).apply {
            setTextColor(0xFFB45309.toInt())
            setPadding(0, dp(8), 0, 0)
        }
        content.add(warning)

        content.add(header("Rules"))
        rows = vertical()
        content.add(rows)
        content.add(button("Add rule") { addRow(FilterRule("sender", "contains", "")); refreshHints() })

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

    /** Fill the editor from the config the server last sent. */
    private fun load() {
        val config = graph.prefs.config
        baseRev = config.filtersRev
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
        adapter = ArrayAdapter(context, android.R.layout.simple_spinner_dropdown_item, options.map { it.second })
        setSelection(options.indexOfFirst { it.first == selected }.coerceAtLeast(0))
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
        val m = MODES[mode.selectedItemPosition].first
        warning.text = when {
            m == "allow" && rows.childCount == 0 -> "With no rules, nothing is forwarded."
            m == "off" && rows.childCount > 0 -> "Rules are ignored while the filter is off."
            else -> ""
        }
        warning.visibility = if (warning.text.isEmpty()) View.GONE else View.VISIBLE
    }

    private fun submit() {
        val filters = current()
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
        b.saveFilters(filters, baseRev)
    }

    private fun render(s: GatewayState) {
        val prev = state
        state = s
        val online = binder != null && s.connection == ConnectionState.CONNECTED
        save.isEnabled = online && s.filtersSave != FiltersSave.Saving
        // The server's answer to our save arrives with (or right after) its new config.
        if (submitted && s.filtersRev != prev.filtersRev) {
            if (s.filtersSave == FiltersSave.Saved) baseRev = s.filtersRev
            if (s.filtersSave == FiltersSave.Conflict) load()
        }
        val f = s.filtersSave
        status.text = when {
            !online -> "Connect to the server to save. The phone keeps using the last filter it received."
            !submitted -> ""
            f == FiltersSave.Saving -> "Saving…"
            f == FiltersSave.Saved -> "Saved."
            f == FiltersSave.Conflict -> "Changed on the web, reloaded. Make your edit again."
            f is FiltersSave.Failed -> "Not saved: ${f.message}"
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
