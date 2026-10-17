package dev.joowdx.rustybee.gateway

import android.Manifest
import android.annotation.SuppressLint
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.PowerManager
import android.telephony.SmsManager
import android.telephony.SubscriptionManager
import dev.joowdx.rustybee.data.Prefs

/** Thin SmsManager wrapper (spec 012 §2–5). */
class AndroidSmsRadio(private val context: Context, private val prefs: Prefs) : SmsRadio {

    override fun canReadSims(): Boolean =
        context.checkSelfPermission(Manifest.permission.READ_PHONE_STATE) == PackageManager.PERMISSION_GRANTED

    @SuppressLint("MissingPermission")
    override fun activeSubscriptionIds(): Set<Int> {
        if (!canReadSims()) return emptySet()
        val sm = context.getSystemService(SubscriptionManager::class.java) ?: return emptySet()
        return sm.activeSubscriptionInfoList.orEmpty().map { it.subscriptionId }.toSet()
    }

    /** Active subscriptions with their slots; empty without READ_PHONE_STATE. */
    @SuppressLint("MissingPermission")
    private fun activeSubs(): List<SimChoice.Sub> {
        if (!canReadSims()) return emptyList()
        val sm = context.getSystemService(SubscriptionManager::class.java) ?: return emptyList()
        return sm.activeSubscriptionInfoList.orEmpty().map { SimChoice.Sub(it.subscriptionId, it.simSlotIndex) }
    }

    private fun manager(sim: Int?): SmsManager {
        val base = context.getSystemService(SmsManager::class.java)
        // With "Ask every time" the plain SmsManager has no subscription to send on (spec 012 §2).
        val id = sim ?: SimChoice.fallback(SubscriptionManager.getDefaultSmsSubscriptionId(), activeSubs())
        return if (id != null) base.createForSubscriptionId(id) else base
    }

    override fun divide(body: String, sim: Int?): List<String> = manager(sim).divideMessage(body).orEmpty()

    override fun send(msgId: String, to: String, parts: List<String>, sim: Int?) {
        val sm = manager(sim)
        val sent = ArrayList<PendingIntent>(parts.size)
        val delivered = ArrayList<PendingIntent>(parts.size)
        parts.indices.forEach { i ->
            sent += pending(SmsStatusReceiver.ACTION_SENT, msgId, i, PendingIntent.FLAG_IMMUTABLE)
            // Mutable so the radio can fill in the `pdu`/`format` extras; the intent is explicit.
            delivered += pending(SmsStatusReceiver.ACTION_DELIVERED, msgId, i, PendingIntent.FLAG_MUTABLE)
        }
        val wl = context.getSystemService(PowerManager::class.java)
            .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "rustybee:send")
        wl.acquire(5_000)
        try {
            if (parts.size == 1) {
                sm.sendTextMessage(to, null, parts[0], sent[0], delivered[0])
            } else {
                sm.sendMultipartTextMessage(to, null, ArrayList(parts), sent, delivered)
            }
            prefs.bump("sends")
        } finally {
            if (wl.isHeld) wl.release()
        }
    }

    private fun pending(action: String, msgId: String, part: Int, mutability: Int): PendingIntent {
        val intent = Intent(context, SmsStatusReceiver::class.java)
            .setAction(action)
            .putExtra(SmsStatusReceiver.EXTRA_MSG_ID, msgId)
            .putExtra(SmsStatusReceiver.EXTRA_PART, part)
        return PendingIntent.getBroadcast(
            context,
            prefs.nextRequestCode(),
            intent,
            mutability or PendingIntent.FLAG_ONE_SHOT,
        )
    }
}
