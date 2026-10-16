package dev.joowdx.rustybee.push

import android.content.Context

/** foss flavor: no push; `fcm_token` is always null (spec 009). */
object Push {
    const val SUPPORTED = false

    fun token(context: Context): String? = null

    fun refresh(context: Context) = Unit
}
