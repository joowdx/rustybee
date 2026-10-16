package dev.joowdx.rustybee.gateway

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.provider.Telephony
import android.telephony.SmsMessage
import android.telephony.SubscriptionManager
import dev.joowdx.rustybee.protocol.Inbound
import kotlin.concurrent.thread

/** Runs [work] off the main thread inside `goAsync()`. */
private fun BroadcastReceiver.async(name: String, work: () -> Unit) {
    val pending = goAsync()
    thread(name = name) {
        try {
            work()
        } finally {
            pending.finish()
        }
    }
}

/** `SMS_RECEIVED`, guarded by `BROADCAST_SMS` in the manifest (spec 012). */
class SmsInboundReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Telephony.Sms.Intents.SMS_RECEIVED_ACTION) return
        val messages = Telephony.Sms.Intents.getMessagesFromIntent(intent) ?: return
        val subIndex = intent.getIntExtra(SubscriptionManager.EXTRA_SUBSCRIPTION_INDEX, -1)
        async("rustybee-inbound") {
            val graph = Graph.of(context)
            val prefs = graph.prefs
            if (!prefs.gatewayEnabled || !prefs.paired) return@async
            val config = prefs.config
            if (!Forwarding.on(config, prefs.forwardingOffHere)) return@async
            val sim = subIndex.takeIf { it >= 0 }
            var queued = false
            messages.filterNotNull()
                .groupBy { it.originatingAddress ?: "" }
                .forEach { (from, pdus) ->
                    val body = pdus.joinToString("") { it.messageBody ?: "" }
                    if (!InboundFilter.forward(config.filters, from, body)) {
                        graph.log("inbound from ${Redact.phone(from)} dropped by filter")
                        return@forEach
                    }
                    val receivedAt = pdus.first().timestampMillis
                    val uid = InboundUid.of(from, receivedAt, body, sim)
                    if (graph.uplink.enqueueInbound(Inbound(uid, from, body, sim, receivedAt))) {
                        prefs.bump("receives")
                        graph.log("inbound from ${Redact.phone(from)} queued")
                        queued = true
                    }
                }
            if (queued) GatewayService.start(context)
        }
    }
}

/** Per-part sent/delivered callbacks from explicit PendingIntents only (exported=false). */
class SmsStatusReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val msgId = intent.getStringExtra(EXTRA_MSG_ID) ?: return
        val code = resultCode
        val action = intent.action
        val pduStatus: Int? = if (action == ACTION_DELIVERED) {
            intent.getByteArrayExtra("pdu")?.let { pdu ->
                runCatching { SmsMessage.createFromPdu(pdu, intent.getStringExtra("format"))?.status }.getOrNull()
            }
        } else {
            null
        }
        async("rustybee-status") {
            val sender = Graph.of(context).sender
            when (action) {
                ACTION_SENT -> sender.onSentResult(msgId, code)
                ACTION_DELIVERED -> sender.onDeliveredResult(msgId, pduStatus ?: if (code == SmsErrors.RESULT_OK) 0 else 0x20)
            }
        }
    }

    companion object {
        const val ACTION_SENT = "dev.joowdx.rustybee.SMS_SENT"
        const val ACTION_DELIVERED = "dev.joowdx.rustybee.SMS_DELIVERED"
        const val EXTRA_MSG_ID = "msg_id"
        const val EXTRA_PART = "part"
    }
}

class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED && intent.action != Intent.ACTION_MY_PACKAGE_REPLACED) return
        val prefs = Graph.of(context).prefs
        if (prefs.paired && prefs.gatewayEnabled) {
            Watchdog.schedule(context)
            GatewayService.start(context)
        }
    }
}
