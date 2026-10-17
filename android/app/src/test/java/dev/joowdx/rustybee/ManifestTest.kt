package dev.joowdx.rustybee

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.w3c.dom.Element
import java.io.File
import javax.xml.parsers.DocumentBuilderFactory

/** AC-012-07 and the spec 011 manifest, checked against this variant's merged manifest. */
class ManifestTest {
    private val ns = "http://schemas.android.com/apk/res/android"

    private val doc by lazy {
        val path = System.getProperty("rustybee.mergedManifest")
        assertNotNull("rustybee.mergedManifest not set", path)
        DocumentBuilderFactory.newInstance().apply { isNamespaceAware = true }
            .newDocumentBuilder().parse(File(path!!))
    }

    private fun elements(tag: String): List<Element> =
        doc.getElementsByTagName(tag).let { nl -> (0 until nl.length).map { nl.item(it) as Element } }

    private fun component(tag: String, name: String): Element =
        elements(tag).single { it.getAttributeNS(ns, "name") == name }

    @Test
    fun ac_012_07_sms_receiver_requires_broadcast_sms() {
        val r = component("receiver", "dev.joowdx.rustybee.gateway.SmsInboundReceiver")
        assertEquals("android.permission.BROADCAST_SMS", r.getAttributeNS(ns, "permission"))
        assertEquals("true", r.getAttributeNS(ns, "exported"))
    }

    @Test
    fun ac_012_07_status_receiver_is_not_exported() {
        val r = component("receiver", "dev.joowdx.rustybee.gateway.SmsStatusReceiver")
        assertEquals("false", r.getAttributeNS(ns, "exported"))
        assertEquals(0, r.getElementsByTagName("intent-filter").length)
    }

    @Test
    fun ac_011_gateway_service_is_remote_messaging_fgs() {
        val s = component("service", "dev.joowdx.rustybee.gateway.GatewayService")
        assertEquals("remoteMessaging", s.getAttributeNS(ns, "foregroundServiceType"))
        val perms = elements("uses-permission").map { it.getAttributeNS(ns, "name") }.toSet()
        listOf(
            "INTERNET", "ACCESS_NETWORK_STATE", "SEND_SMS", "RECEIVE_SMS", "READ_PHONE_STATE", "POST_NOTIFICATIONS",
            "FOREGROUND_SERVICE", "FOREGROUND_SERVICE_REMOTE_MESSAGING", "RECEIVE_BOOT_COMPLETED",
            "REQUEST_IGNORE_BATTERY_OPTIMIZATIONS", "WAKE_LOCK", "CAMERA",
        ).forEach { assertTrue(it, "android.permission.$it" in perms) }
    }

    @Test
    fun ac_015_network_screen_and_location_permissions() {
        assertEquals("false", component("activity", "dev.joowdx.rustybee.ui.NetworkActivity").getAttributeNS(ns, "exported"))
        val perms = elements("uses-permission").map { it.getAttributeNS(ns, "name") }.toSet()
        listOf("ACCESS_COARSE_LOCATION", "ACCESS_FINE_LOCATION", "ACCESS_BACKGROUND_LOCATION")
            .forEach { assertTrue(it, "android.permission.$it" in perms) }
    }

    @Test
    fun camera_is_optional() {
        val camera = elements("uses-feature").single { it.getAttributeNS(ns, "name") == "android.hardware.camera.any" }
        assertEquals("false", camera.getAttributeNS(ns, "required"))
    }
}
