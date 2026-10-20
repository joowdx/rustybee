package dev.joowdx.rustybee.gateway

import android.Manifest

/** The runtime permissions the app asks for, by API level: notifications need one from API 33 (spec 011). */
object RuntimePermissions {
    val SMS = arrayOf(
        Manifest.permission.SEND_SMS,
        Manifest.permission.RECEIVE_SMS,
        Manifest.permission.READ_PHONE_STATE,
    )

    const val NOTIFICATIONS = Manifest.permission.POST_NOTIFICATIONS

    fun notificationsNeedPermission(sdk: Int): Boolean = sdk >= 33

    fun forSdk(sdk: Int): Array<String> = if (notificationsNeedPermission(sdk)) SMS + NOTIFICATIONS else SMS

    /** Below API 33 there is no permission to grant, so only the app's notification settings decide. */
    fun notificationsGranted(sdk: Int, granted: (String) -> Boolean): Boolean =
        !notificationsNeedPermission(sdk) || granted(NOTIFICATIONS)
}
