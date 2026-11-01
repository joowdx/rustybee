package dev.joowdx.rustybee.ui

import android.app.Activity
import android.os.Bundle
import android.view.View
import android.widget.ArrayAdapter
import android.widget.RadioButton
import android.widget.RadioGroup
import android.widget.Spinner
import android.widget.AdapterView
import dev.joowdx.rustybee.gateway.GatewayService
import dev.joowdx.rustybee.gateway.Graph
import dev.joowdx.rustybee.gateway.StatsPeriod
import java.time.DayOfWeek
import java.time.format.TextStyle
import java.util.Locale

/** What the gateway notification counts (spec 016). Phone-only settings. */
class NotificationActivity : Activity() {
    private val prefs by lazy { Graph.of(this).prefs }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val content = vertical()
        content.add(text("Notification", 24f, bold = true))
        content.add(
            text("Choose the period the gateway notification counts. Counts come from your server, and from this phone while offline.").apply {
                setPadding(0, dp(8), 0, 0)
            },
        )

        content.add(header("Show counts for"))
        val ids = mutableMapOf<Int, StatsPeriod>()
        val group = RadioGroup(this)
        val current = prefs.statsPeriod
        StatsPeriod.entries.forEach { period ->
            val button = RadioButton(this).apply {
                id = View.generateViewId()
                text = period.label
                textSize = 16f
            }
            ids[button.id] = period
            group.addView(button)
            if (period == current) group.check(button.id)
        }
        content.add(group)

        val weekHeader = header("Week starts on")
        // Monday first, as java.time numbers the days.
        val days = DayOfWeek.entries
        val week = Spinner(this).apply {
            adapter = ArrayAdapter(
                this@NotificationActivity,
                android.R.layout.simple_spinner_dropdown_item,
                days.map { it.getDisplayName(TextStyle.FULL, Locale.getDefault()) },
            )
            setSelection(days.indexOf(prefs.weekStart))
            onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
                override fun onItemSelected(parent: AdapterView<*>?, view: View?, position: Int, id: Long) {
                    if (days[position] == prefs.weekStart) return
                    prefs.weekStart = days[position]
                    GatewayService.onStatsSettings()
                }

                override fun onNothingSelected(parent: AdapterView<*>?) = Unit
            }
        }
        content.add(weekHeader).add(week)

        fun showWeek(period: StatsPeriod) {
            val v = if (period == StatsPeriod.WEEK) View.VISIBLE else View.GONE
            weekHeader.visibility = v
            week.visibility = v
        }
        showWeek(current)
        group.setOnCheckedChangeListener { _, id ->
            val period = ids[id] ?: return@setOnCheckedChangeListener
            prefs.statsPeriod = period
            showWeek(period)
            GatewayService.onStatsSettings()
        }
        scrollingRoot(content)
    }
}
