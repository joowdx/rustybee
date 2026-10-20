package dev.joowdx.rustybee

import dev.joowdx.rustybee.gateway.RuntimePermissions
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** AC-011-13: Android 11 to 12L has no notification permission to ask for. */
class RuntimePermissionsTest {
    @Test
    fun ac_011_13_no_notification_permission_below_33() {
        assertFalse(RuntimePermissions.NOTIFICATIONS in RuntimePermissions.forSdk(30))
        assertFalse(RuntimePermissions.NOTIFICATIONS in RuntimePermissions.forSdk(32))
        assertTrue(RuntimePermissions.notificationsGranted(30) { false })
    }

    @Test
    fun ac_011_13_notification_permission_from_33() {
        assertTrue(RuntimePermissions.NOTIFICATIONS in RuntimePermissions.forSdk(33))
        assertFalse(RuntimePermissions.notificationsGranted(36) { false })
        assertTrue(RuntimePermissions.notificationsGranted(36) { true })
    }

    @Test
    fun ac_011_13_sms_permissions_on_every_level() {
        listOf(30, 33, 36).forEach { sdk ->
            assertTrue(RuntimePermissions.SMS.all { it in RuntimePermissions.forSdk(sdk) })
        }
    }
}
