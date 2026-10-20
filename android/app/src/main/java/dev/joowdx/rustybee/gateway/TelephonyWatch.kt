package dev.joowdx.rustybee.gateway

import android.annotation.SuppressLint
import android.annotation.TargetApi
import android.os.Build
import android.telephony.PhoneStateListener
import android.telephony.ServiceState
import android.telephony.SignalStrength
import android.telephony.TelephonyCallback
import android.telephony.TelephonyDisplayInfo
import android.telephony.TelephonyManager
import java.util.concurrent.Executor

/**
 * One telephony listener of a fixed [Watch]: a [TelephonyCallback] on API 31+, a
 * [PhoneStateListener] on Android 11 (spec 011). [onEvent] gets this watch, so a callback
 * already queued from a replaced watch can be told apart.
 */
class TelephonyWatch(val watch: Watch, private val onEvent: (TelephonyWatch, Event) -> Unit) {
    /** The listener sets the service uses, with their Android 11 `LISTEN_*` flags. */
    @Suppress("DEPRECATION")
    enum class Watch(val listenEvents: Int) {
        SERVICE_STATE(PhoneStateListener.LISTEN_SERVICE_STATE),
        SERVICE_STATE_AND_SIGNAL(PhoneStateListener.LISTEN_SERVICE_STATE or PhoneStateListener.LISTEN_SIGNAL_STRENGTHS),
        DISPLAY_INFO_AND_MOBILE_DATA(PhoneStateListener.LISTEN_DISPLAY_INFO_CHANGED or PhoneStateListener.LISTEN_USER_MOBILE_DATA_STATE),
    }

    sealed interface Event {
        data object Service : Event
        data class Signal(val strength: SignalStrength) : Event
        data class Display(val info: TelephonyDisplayInfo) : Event
        data object MobileData : Event
    }

    private var stopper: (() -> Unit)? = null

    /** Registers on [tm]; throws what the platform throws (e.g. a missing permission). */
    fun start(tm: TelephonyManager, executor: Executor) {
        stopper = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) startCallback(tm, executor) else startListener(tm, executor)
    }

    fun stop() {
        stopper?.let { runCatching(it) }
        stopper = null
    }

    private fun emit(e: Event) = onEvent(this, e)

    // A TelephonyCallback registers for exactly the listener interfaces it implements, so each
    // [Watch] has its own class.
    @TargetApi(Build.VERSION_CODES.S)
    @SuppressLint("MissingPermission")
    private fun startCallback(tm: TelephonyManager, executor: Executor): () -> Unit {
        val cb: TelephonyCallback = when (watch) {
            Watch.SERVICE_STATE -> object : TelephonyCallback(), TelephonyCallback.ServiceStateListener {
                override fun onServiceStateChanged(serviceState: ServiceState) = emit(Event.Service)
            }
            Watch.SERVICE_STATE_AND_SIGNAL ->
                object : TelephonyCallback(), TelephonyCallback.ServiceStateListener, TelephonyCallback.SignalStrengthsListener {
                    override fun onServiceStateChanged(serviceState: ServiceState) = emit(Event.Service)
                    override fun onSignalStrengthsChanged(signalStrength: SignalStrength) = emit(Event.Signal(signalStrength))
                }
            Watch.DISPLAY_INFO_AND_MOBILE_DATA ->
                object : TelephonyCallback(), TelephonyCallback.DisplayInfoListener, TelephonyCallback.UserMobileDataStateListener {
                    override fun onDisplayInfoChanged(telephonyDisplayInfo: TelephonyDisplayInfo) = emit(Event.Display(telephonyDisplayInfo))
                    override fun onUserMobileDataStateChanged(enabled: Boolean) = emit(Event.MobileData)
                }
        }
        tm.registerTelephonyCallback(executor, cb)
        return { tm.unregisterTelephonyCallback(cb) }
    }

    @Suppress("DEPRECATION")
    @SuppressLint("MissingPermission")
    private fun startListener(tm: TelephonyManager, executor: Executor): () -> Unit {
        val listener = object : PhoneStateListener(executor) {
            override fun onServiceStateChanged(serviceState: ServiceState?) = emit(Event.Service)
            override fun onSignalStrengthsChanged(signalStrength: SignalStrength?) {
                if (signalStrength != null) emit(Event.Signal(signalStrength))
            }
            override fun onDisplayInfoChanged(telephonyDisplayInfo: TelephonyDisplayInfo) = emit(Event.Display(telephonyDisplayInfo))
            override fun onUserMobileDataStateChanged(enabled: Boolean) = emit(Event.MobileData)
        }
        tm.listen(listener, watch.listenEvents)
        return { tm.listen(listener, PhoneStateListener.LISTEN_NONE) }
    }
}
