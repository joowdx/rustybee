package dev.joowdx.rustybee.ui

import android.app.Activity
import android.content.Context
import android.graphics.Typeface
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.view.WindowInsets
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView

/** Small helpers for code-built platform Views. */
internal fun Context.dp(v: Int): Int =
    TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v.toFloat(), resources.displayMetrics).toInt()

internal fun Context.vertical(): LinearLayout = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }

internal fun Context.horizontal(): LinearLayout = LinearLayout(this).apply {
    orientation = LinearLayout.HORIZONTAL
    gravity = Gravity.CENTER_VERTICAL
}

internal fun Context.text(value: CharSequence = "", sizeSp: Float = 15f, bold: Boolean = false): TextView =
    TextView(this).apply {
        text = value
        setTextSize(TypedValue.COMPLEX_UNIT_SP, sizeSp)
        if (bold) setTypeface(typeface, Typeface.BOLD)
        setTextIsSelectable(false)
    }

internal fun Context.header(value: String): TextView = text(value, 13f, bold = true).apply {
    isAllCaps = true
    setPadding(0, dp(24), 0, dp(8))
    alpha = 0.7f
}

internal fun Context.button(label: String, onClick: () -> Unit): Button = Button(this).apply {
    text = label
    isAllCaps = false
    setOnClickListener { onClick() }
}

internal fun LinearLayout.add(view: View, weight: Float = 0f): LinearLayout {
    val lp = if (orientation == LinearLayout.VERTICAL) {
        LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)
    } else {
        LinearLayout.LayoutParams(if (weight > 0f) 0 else ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT, weight)
    }
    addView(view, lp)
    return this
}

/** Edge-to-edge: a scrolling root whose content is padded by system bars, cutouts and the IME. */
internal fun Activity.scrollingRoot(content: LinearLayout): ScrollView {
    val pad = dp(16)
    val scroll = ScrollView(this).apply {
        isFillViewport = true
        addView(content, ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))
    }
    scroll.setOnApplyWindowInsetsListener { v, insets ->
        val bars = insets.getInsets(
            WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout() or WindowInsets.Type.ime(),
        )
        content.setPadding(bars.left + pad, bars.top + pad, bars.right + pad, bars.bottom + pad)
        insets
    }
    setContentView(scroll)
    return scroll
}
