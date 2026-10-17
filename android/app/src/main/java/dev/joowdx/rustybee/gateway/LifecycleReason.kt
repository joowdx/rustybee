package dev.joowdx.rustybee.gateway

/**
 * Spec 002 strings for the platform start and exit reason ints
 * (`START_REASON_*` and `REASON_*`). No Android types, so the JVM tests can call this.
 */
object LifecycleReason {
    fun startReason(reason: Int): String = when (reason) {
        0 -> "alarm"
        1 -> "backup"
        2 -> "boot_complete"
        3 -> "broadcast"
        4 -> "content_provider"
        5 -> "job"
        6 -> "launcher"
        7 -> "launcher_recents"
        8 -> "other"
        9 -> "push"
        10 -> "service"
        11 -> "start_activity"
        else -> "unknown_$reason"
    }

    fun exitReason(reason: Int): String = when (reason) {
        0 -> "unknown"
        1 -> "exit_self"
        2 -> "signaled"
        3 -> "low_memory"
        4 -> "crash"
        5 -> "crash_native"
        6 -> "anr"
        7 -> "initialization_failure"
        8 -> "permission_change"
        9 -> "excessive_resource_usage"
        10 -> "user_requested"
        11 -> "user_stopped"
        12 -> "dependency_died"
        13 -> "other"
        14 -> "freezer"
        15 -> "package_state_change"
        16 -> "package_updated"
        else -> "unknown_$reason"
    }

    /** OS description, cut to 200 characters. Empty text is null. */
    fun description(text: String?): String? = text?.take(DESCRIPTION_MAX)?.ifEmpty { null }

    /**
     * `START_TIMESTAMP_LAUNCH` is a clock-monotonic nanosecond timestamp.
     * Spec 002 wants epoch milliseconds. Returns null when the OS omitted the timestamp.
     */
    fun launchEpochMs(launchMonoNs: Long?, nowMonoNs: Long, nowEpochMs: Long): Long? =
        launchMonoNs?.let { nowEpochMs - (nowMonoNs - it) / 1_000_000L }

    private const val DESCRIPTION_MAX = 200
}
