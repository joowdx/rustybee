package dev.joowdx.rustybee.protocol

import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/** Spec 002 wire format. Unknown fields are ignored; null optionals are omitted on output. */
val ProtocolJson: Json = Json {
    ignoreUnknownKeys = true
    explicitNulls = false
    encodeDefaults = true
}

const val PROTOCOL_VERSION = 1

object FrameType {
    const val HELLO = "hello"
    const val STATUS = "status"
    const val INBOUND = "inbound"
    const val HEALTH = "health"
    const val WELCOME = "welcome"
    const val CONFIG = "config"
    const val SEND = "send"
    const val ACK = "ack"
    const val NACK = "nack"
    const val SET_FORWARDING = "set_forwarding"

    val ALL = setOf(HELLO, STATUS, INBOUND, HEALTH, WELCOME, CONFIG, SEND, ACK, NACK, SET_FORWARDING)
}

@Serializable
data class Envelope(
    val v: Int,
    val id: String,
    val type: String,
    val data: JsonElement = JsonObject(emptyMap()),
)

@Serializable
data class Ack(val ref: String)

@Serializable
data class Nack(val ref: String, val code: String, val message: String? = null)

@Serializable
data class SimInfo(
    @SerialName("subscription_id") val subscriptionId: Int,
    val slot: Int,
    val carrier: String? = null,
    val number: String? = null,
)

@Serializable
data class Battery(val level: Int, val charging: Boolean)

@Serializable
data class Permissions(
    @SerialName("send_sms") val sendSms: Boolean,
    @SerialName("receive_sms") val receiveSms: Boolean,
    val notifications: Boolean,
    @SerialName("battery_unrestricted") val batteryUnrestricted: Boolean,
    /** Optional (spec 002): older apps omit it. Without it the SIM list is empty and an explicit `sim` fails. */
    @SerialName("read_phone_state") val readPhoneState: Boolean? = null,
)

@Serializable
data class LifecycleStart(
    val reason: String,
    val at: Long,
    @SerialName("was_force_stopped") val wasForceStopped: Boolean,
)

@Serializable
data class LifecycleExit(
    val reason: String,
    val at: Long,
    val description: String? = null,
)

/** How this process started, and why the previous one ended. Either side may be absent. */
@Serializable
data class Lifecycle(
    val start: LifecycleStart? = null,
    val exit: LifecycleExit? = null,
)

@Serializable
data class Hello(
    @SerialName("app_version") val appVersion: String,
    @SerialName("app_build") val appBuild: Int,
    @SerialName("os_version") val osVersion: String,
    val manufacturer: String,
    val model: String,
    val sims: List<SimInfo>,
    val battery: Battery,
    val network: String,
    @SerialName("fcm_token") val fcmToken: String? = null,
    val permissions: Permissions,
    val outbox: Int,
    val lifecycle: Lifecycle? = null,
)

@Serializable
data class Health(
    val battery: Battery,
    val network: String,
    val permissions: Permissions,
    val outbox: Int,
    @SerialName("fcm_token") val fcmToken: String? = null,
)

@Serializable
data class StatusError(val code: String, val message: String)

@Serializable
data class Status(
    @SerialName("msg_id") val msgId: String,
    val state: String,
    val at: Long,
    val parts: Int,
    val error: StatusError? = null,
)

@Serializable
data class Inbound(
    val uid: String,
    val from: String,
    val body: String,
    val sim: Int? = null,
    @SerialName("received_at") val receivedAt: Long,
)

@Serializable
data class FilterRule(
    val field: String,
    val op: String,
    val value: String,
    @SerialName("ignore_case") val ignoreCase: Boolean = true,
)

@Serializable
data class Filters(val mode: String = "off", val rules: List<FilterRule> = emptyList())

@Serializable
data class Config(
    val name: String = "",
    @SerialName("ping_s") val pingS: Int = DEFAULT_PING_S,
    @SerialName("receive_enabled") val receiveEnabled: Boolean = true,
    val sim: Int? = null,
    val filters: Filters = Filters(),
    @SerialName("filters_rev") val filtersRev: Long = 0,
) {
    companion object {
        const val DEFAULT_PING_S = 270
    }
}

/** Forwarding edited on the phone; [baseRev] is the `filters_rev` it was based on. */
@Serializable
data class SetForwarding(
    @SerialName("base_rev") val baseRev: Long,
    @SerialName("receive_enabled") val receiveEnabled: Boolean,
    val filters: Filters,
)

@Serializable
data class Welcome(
    @SerialName("device_id") val deviceId: String,
    @SerialName("server_time") val serverTime: Long,
    val config: Config,
)

@Serializable
data class Send(
    @SerialName("msg_id") val msgId: String,
    val to: String,
    val body: String,
    val sim: Int? = null,
)

@Serializable
data class PairRequest(
    val code: String,
    val name: String,
    val manufacturer: String,
    val model: String,
    @SerialName("os_version") val osVersion: String,
    @SerialName("app_version") val appVersion: String,
    @SerialName("app_build") val appBuild: Int,
    @SerialName("fcm_token") val fcmToken: String? = null,
)

@Serializable
data class PairResponse(
    @SerialName("device_id") val deviceId: String,
    @SerialName("device_token") val deviceToken: String,
    @SerialName("ws_url") val wsUrl: String,
)

@Serializable
data class ApiError(val code: String, val message: String? = null)

@Serializable
data class ApiErrorBody(val error: ApiError)

object Frames {
    /** Builds one text frame with a fresh ULID. */
    fun <T> encode(type: String, serializer: KSerializer<T>, data: T, id: String = Ulid.next()): String =
        ProtocolJson.encodeToString(
            Envelope.serializer(),
            Envelope(PROTOCOL_VERSION, id, type, ProtocolJson.encodeToJsonElement(serializer, data)),
        )

    fun ack(ref: String): String = encode(FrameType.ACK, Ack.serializer(), Ack(ref))

    fun nack(ref: String, code: String, message: String): String =
        encode(FrameType.NACK, Nack.serializer(), Nack(ref, code, message))

    /** The serializer for a frame type's `data`, or null for an unknown type. */
    fun dataSerializer(type: String): KSerializer<*>? = when (type) {
        FrameType.HELLO -> Hello.serializer()
        FrameType.STATUS -> Status.serializer()
        FrameType.INBOUND -> Inbound.serializer()
        FrameType.HEALTH -> Health.serializer()
        FrameType.WELCOME -> Welcome.serializer()
        FrameType.CONFIG -> Config.serializer()
        FrameType.SEND -> Send.serializer()
        FrameType.ACK -> Ack.serializer()
        FrameType.NACK -> Nack.serializer()
        FrameType.SET_FORWARDING -> SetForwarding.serializer()
        else -> null
    }
}
