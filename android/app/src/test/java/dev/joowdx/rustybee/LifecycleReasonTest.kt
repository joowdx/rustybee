package dev.joowdx.rustybee

import android.app.ApplicationExitInfo
import android.app.ApplicationStartInfo
import dev.joowdx.rustybee.gateway.LifecycleReason
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** AC-011-09. */
class LifecycleReasonTest {
    @Test
    fun ac_011_09_every_start_reason_constant() {
        val expected = mapOf(
            ApplicationStartInfo.START_REASON_ALARM to "alarm",
            ApplicationStartInfo.START_REASON_BACKUP to "backup",
            ApplicationStartInfo.START_REASON_BOOT_COMPLETE to "boot_complete",
            ApplicationStartInfo.START_REASON_BROADCAST to "broadcast",
            ApplicationStartInfo.START_REASON_CONTENT_PROVIDER to "content_provider",
            ApplicationStartInfo.START_REASON_JOB to "job",
            ApplicationStartInfo.START_REASON_LAUNCHER to "launcher",
            ApplicationStartInfo.START_REASON_LAUNCHER_RECENTS to "launcher_recents",
            ApplicationStartInfo.START_REASON_OTHER to "other",
            ApplicationStartInfo.START_REASON_PUSH to "push",
            ApplicationStartInfo.START_REASON_SERVICE to "service",
            ApplicationStartInfo.START_REASON_START_ACTIVITY to "start_activity",
        )
        val fields = ApplicationStartInfo::class.java.fields.filter { it.name.startsWith("START_REASON_") }
        assertEquals(expected.size, fields.size)
        for (field in fields) {
            val value = field.getInt(null)
            assertEquals(field.name, expected.getValue(value), LifecycleReason.startReason(value))
        }
    }

    @Test
    fun ac_011_09_every_exit_reason_constant() {
        val expected = mapOf(
            ApplicationExitInfo.REASON_UNKNOWN to "unknown",
            ApplicationExitInfo.REASON_EXIT_SELF to "exit_self",
            ApplicationExitInfo.REASON_SIGNALED to "signaled",
            ApplicationExitInfo.REASON_LOW_MEMORY to "low_memory",
            ApplicationExitInfo.REASON_CRASH to "crash",
            ApplicationExitInfo.REASON_CRASH_NATIVE to "crash_native",
            ApplicationExitInfo.REASON_ANR to "anr",
            ApplicationExitInfo.REASON_INITIALIZATION_FAILURE to "initialization_failure",
            ApplicationExitInfo.REASON_PERMISSION_CHANGE to "permission_change",
            ApplicationExitInfo.REASON_EXCESSIVE_RESOURCE_USAGE to "excessive_resource_usage",
            ApplicationExitInfo.REASON_USER_REQUESTED to "user_requested",
            ApplicationExitInfo.REASON_USER_STOPPED to "user_stopped",
            ApplicationExitInfo.REASON_DEPENDENCY_DIED to "dependency_died",
            ApplicationExitInfo.REASON_OTHER to "other",
            ApplicationExitInfo.REASON_FREEZER to "freezer",
            ApplicationExitInfo.REASON_PACKAGE_STATE_CHANGE to "package_state_change",
            ApplicationExitInfo.REASON_PACKAGE_UPDATED to "package_updated",
        )
        val fields = ApplicationExitInfo::class.java.fields.filter { it.name.startsWith("REASON_") }
        assertEquals(expected.size, fields.size)
        for (field in fields) {
            val value = field.getInt(null)
            assertEquals(field.name, expected.getValue(value), LifecycleReason.exitReason(value))
        }
        assertEquals("unknown", LifecycleReason.exitReason(ApplicationExitInfo.REASON_UNKNOWN))
    }

    @Test
    fun ac_011_09_unknown_int() {
        assertEquals("unknown_12", LifecycleReason.startReason(12))
        assertEquals("unknown_99", LifecycleReason.exitReason(99))
        assertEquals("unknown_-1", LifecycleReason.startReason(-1))
        assertEquals("unknown_-1", LifecycleReason.exitReason(-1))
    }

    @Test
    fun ac_011_09_description_truncated_to_200() {
        assertNull(LifecycleReason.description(null))
        assertNull(LifecycleReason.description(""))
        assertEquals("ok", LifecycleReason.description("ok"))
        val exact = "a".repeat(200)
        assertEquals(exact, LifecycleReason.description(exact))
        assertEquals(exact, LifecycleReason.description(exact + "b"))
    }
}
