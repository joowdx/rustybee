package dev.joowdx.rustybee.gateway

import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.ComponentName
import android.content.Context

/** Periodic (15 min) JobScheduler check that the gateway is still running (spec 011). */
class Watchdog : JobService() {
    override fun onStartJob(params: JobParameters): Boolean {
        val prefs = Graph.of(this).prefs
        if (prefs.paired && prefs.gatewayEnabled && !GatewayService.running) {
            val started = Telemetry(this).batteryUnrestricted() && GatewayService.start(this)
            if (!started) Notifications.postStopped(this)
        }
        return false
    }

    override fun onStopJob(params: JobParameters): Boolean = false

    companion object {
        private const val JOB_ID = 1

        fun schedule(context: Context) {
            val js = context.getSystemService(JobScheduler::class.java)
            if (js.getPendingJob(JOB_ID) != null) return
            js.schedule(
                JobInfo.Builder(JOB_ID, ComponentName(context, Watchdog::class.java))
                    .setPeriodic(15 * 60_000L)
                    .setRequiredNetworkType(JobInfo.NETWORK_TYPE_ANY)
                    .setPersisted(true)
                    .build(),
            )
        }

        fun cancel(context: Context) {
            context.getSystemService(JobScheduler::class.java).cancel(JOB_ID)
        }
    }
}
