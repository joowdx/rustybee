package dev.joowdx.rustybee.ui

import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.graphics.Typeface
import android.os.Bundle
import android.view.ViewGroup
import android.view.WindowInsets
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import dev.joowdx.rustybee.gateway.Graph
import java.text.DateFormat
import java.util.Date

/** The device's recent log lines, with copy and clear. */
class LogsActivity : Activity() {
    private val graph by lazy { Graph.of(this) }
    private lateinit var log: TextView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val content = vertical()
        content.add(text("Logs", 24f, bold = true))
        content.add(
            horizontal()
                .add(button("Copy") { copyLog() })
                .add(button("Clear") { graph.db.clearLog(); refreshLog() })
                .apply { setPadding(0, dp(8), 0, 0) },
        )
        log = text("", 12f).apply {
            typeface = Typeface.MONOSPACE
            setTextIsSelectable(true)
            setPadding(0, dp(8), 0, 0)
        }
        // Only the log scrolls; the title and buttons stay pinned above it.
        content.addView(
            ScrollView(this).apply { addView(log) },
            LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f),
        )
        val pad = dp(16)
        content.setOnApplyWindowInsetsListener { v, insets ->
            val bars = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout())
            v.setPadding(bars.left + pad, bars.top + pad, bars.right + pad, bars.bottom + pad)
            insets
        }
        setContentView(content)
    }

    override fun onResume() {
        super.onResume()
        refreshLog()
    }

    private fun refreshLog() {
        val fmt = DateFormat.getTimeInstance(DateFormat.MEDIUM)
        log.text = graph.db.recentLog(100).joinToString("\n") { "${fmt.format(Date(it.at))} ${it.level} ${it.msg}" }
            .ifEmpty { "(empty)" }
    }

    private fun copyLog() {
        getSystemService(ClipboardManager::class.java)
            .setPrimaryClip(ClipData.newPlainText("RustyBee log", log.text))
    }
}
