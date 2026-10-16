package dev.joowdx.rustybee.ui

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.ImageFormat
import android.graphics.Matrix
import android.graphics.RectF
import android.graphics.SurfaceTexture
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.params.OutputConfiguration
import android.hardware.camera2.params.SessionConfiguration
import android.media.ImageReader
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.util.Size
import android.view.Gravity
import android.view.Surface
import android.view.TextureView
import android.view.ViewGroup
import android.view.WindowInsets
import android.widget.FrameLayout
import android.widget.TextView
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.ReaderException
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import java.util.concurrent.Executor
import kotlin.math.max

/**
 * Scans the pairing QR code from the web UI with the platform camera (spec 013) and returns the
 * `rustybee://pair` link as the result's data. Camera2 + ZXing core: no Play services, no AndroidX.
 */
class ScanActivity : Activity() {
    private lateinit var preview: TextureView
    private lateinit var hint: TextView

    private var thread: HandlerThread? = null
    private var handler: Handler? = null
    private var camera: CameraDevice? = null
    private var session: CameraCaptureSession? = null
    private var reader: ImageReader? = null
    private var previewSize = Size(1280, 720)
    @Volatile private var done = false

    private val reader2d = QRCodeReader()
    private val hints = mapOf(
        DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE),
        DecodeHintType.TRY_HARDER to true,
    )

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        preview = TextureView(this)
        hint = text("Point the camera at the pairing QR code on the server's Devices page.", 16f).apply {
            setTextColor(0xFFFFFFFF.toInt())
            setBackgroundColor(0x99000000.toInt())
            gravity = Gravity.CENTER
            setPadding(dp(16), dp(12), dp(16), dp(12))
        }
        val cancel = button("Cancel") { finish() }
        val overlay = vertical().apply { gravity = Gravity.BOTTOM }
        overlay.add(hint).add(cancel)
        val root = FrameLayout(this).apply {
            setBackgroundColor(0xFF000000.toInt())
            addView(preview, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
            addView(overlay, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        }
        root.setOnApplyWindowInsetsListener { _, insets ->
            val bars = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout())
            val pad = dp(16)
            overlay.setPadding(bars.left + pad, bars.top + pad, bars.right + pad, bars.bottom + pad)
            insets
        }
        setContentView(root)
        if (savedInstanceState == null && !granted()) requestPermissions(arrayOf(Manifest.permission.CAMERA), REQ_CAMERA)
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != REQ_CAMERA) return
        if (grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED) {
            start()
        } else {
            setResult(RESULT_DENIED)
            finish()
        }
    }

    override fun onResume() {
        super.onResume()
        if (granted()) start()
    }

    override fun onPause() {
        stop()
        super.onPause()
    }

    private fun granted() = checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED

    private fun start() {
        if (thread != null) return
        thread = HandlerThread("scan").also { it.start() }
        handler = Handler(thread!!.looper)
        if (preview.isAvailable) {
            open()
        } else {
            preview.surfaceTextureListener = object : TextureView.SurfaceTextureListener {
                override fun onSurfaceTextureAvailable(s: SurfaceTexture, w: Int, h: Int) = open()
                override fun onSurfaceTextureSizeChanged(s: SurfaceTexture, w: Int, h: Int) = transform()
                override fun onSurfaceTextureDestroyed(s: SurfaceTexture) = true
                override fun onSurfaceTextureUpdated(s: SurfaceTexture) = Unit
            }
        }
    }

    private fun stop() {
        session?.close()
        session = null
        camera?.close()
        camera = null
        reader?.close()
        reader = null
        thread?.quitSafely()
        thread = null
        handler = null
    }

    private fun fail(message: String) = runOnUiThread { hint.text = message }

    @SuppressLint("MissingPermission") // checked in start()'s callers
    private fun open() {
        if (thread == null || camera != null || !granted()) return
        val manager = getSystemService(CameraManager::class.java)
        val id = manager.cameraIdList.firstOrNull {
            manager.getCameraCharacteristics(it).get(CameraCharacteristics.LENS_FACING) == CameraCharacteristics.LENS_FACING_BACK
        } ?: manager.cameraIdList.firstOrNull()
            ?: return fail("This phone has no camera. Go back and paste the pairing link instead.")
        val map = manager.getCameraCharacteristics(id).get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
        previewSize = map?.getOutputSizes(ImageFormat.YUV_420_888)
            ?.filter { max(it.width, it.height) <= 1280 }
            ?.maxByOrNull { it.width * it.height }
            ?: previewSize
        transform()
        try {
            manager.openCamera(id, object : CameraDevice.StateCallback() {
                override fun onOpened(device: CameraDevice) {
                    camera = device
                    startSession(device)
                }

                override fun onDisconnected(device: CameraDevice) {
                    device.close()
                    if (camera == device) camera = null
                }

                override fun onError(device: CameraDevice, error: Int) {
                    device.close()
                    if (camera == device) camera = null
                    fail("The camera is unavailable (error $error). Go back and paste the pairing link instead.")
                }
            }, handler)
        } catch (e: Exception) {
            fail("The camera is unavailable: ${e.message ?: e.javaClass.simpleName}")
        }
    }

    private fun startSession(device: CameraDevice) {
        val h = handler ?: return
        val texture = preview.surfaceTexture ?: return
        texture.setDefaultBufferSize(previewSize.width, previewSize.height)
        val surface = Surface(texture)
        val frames = ImageReader.newInstance(previewSize.width, previewSize.height, ImageFormat.YUV_420_888, 2)
        frames.setOnImageAvailableListener({ decode(it) }, h)
        reader = frames
        val executor = Executor { h.post(it) }
        val config = SessionConfiguration(
            SessionConfiguration.SESSION_REGULAR,
            listOf(OutputConfiguration(surface), OutputConfiguration(frames.surface)),
            executor,
            object : CameraCaptureSession.StateCallback() {
                override fun onConfigured(s: CameraCaptureSession) {
                    if (camera != device) return s.close()
                    session = s
                    val request = device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW).apply {
                        addTarget(surface)
                        addTarget(frames.surface)
                        set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_PICTURE)
                    }.build()
                    runCatching { s.setRepeatingRequest(request, null, h) }
                        .onFailure { fail("The camera stopped: ${it.message}") }
                }

                override fun onConfigureFailed(s: CameraCaptureSession) =
                    fail("The camera could not start. Go back and paste the pairing link instead.")
            },
        )
        runCatching { device.createCaptureSession(config) }.onFailure { fail("The camera could not start: ${it.message}") }
    }

    /** Decode the luminance (Y) plane; QR codes read the same at any rotation. */
    private fun decode(r: ImageReader) {
        val image = r.acquireLatestImage() ?: return
        val text = image.use {
            if (done) return
            val plane = it.planes[0]
            val buf = plane.buffer
            val data = ByteArray(plane.rowStride * it.height)
            buf.get(data, 0, minOf(buf.remaining(), data.size))
            val source = PlanarYUVLuminanceSource(data, plane.rowStride, it.height, 0, 0, it.width, it.height, false)
            try {
                reader2d.decode(BinaryBitmap(HybridBinarizer(source)), hints).text
            } catch (_: ReaderException) {
                null
            } finally {
                reader2d.reset()
            }
        } ?: return
        if (!isPairLink(text)) {
            runOnUiThread { hint.text = "That QR code isn't a RustyBee pairing code. Use the one on the server's Devices page." }
            return
        }
        done = true
        runOnUiThread {
            setResult(RESULT_OK, Intent().setData(Uri.parse(text.trim())))
            finish()
        }
    }

    /** Rotate and centre-crop the camera buffer to fill the view (Camera2 does not do this for TextureView). */
    private fun transform() {
        val vw = preview.width.toFloat()
        val vh = preview.height.toFloat()
        if (vw == 0f || vh == 0f) return
        val rotation = display?.rotation ?: Surface.ROTATION_0
        val pw = previewSize.width.toFloat()
        val ph = previewSize.height.toFloat()
        val cx = vw / 2
        val cy = vh / 2
        val matrix = Matrix()
        if (rotation == Surface.ROTATION_90 || rotation == Surface.ROTATION_270) {
            val view = RectF(0f, 0f, vw, vh)
            val buffer = RectF(0f, 0f, ph, pw).apply { offset(cx - centerX(), cy - centerY()) }
            matrix.setRectToRect(view, buffer, Matrix.ScaleToFit.FILL)
            val scale = max(vh / ph, vw / pw)
            matrix.postScale(scale, scale, cx, cy)
            matrix.postRotate(90f * (rotation - 2), cx, cy)
        } else {
            // Upright: the buffer is shown as ph × pw (portrait), stretched to the view; undo the stretch.
            val scale = max(vw / ph, vh / pw)
            matrix.setScale(ph * scale / vw, pw * scale / vh, cx, cy)
            if (rotation == Surface.ROTATION_180) matrix.postRotate(180f, cx, cy)
        }
        runOnUiThread { preview.setTransform(matrix) }
    }

    companion object {
        /** Result code when the camera permission was refused. */
        const val RESULT_DENIED = RESULT_FIRST_USER
        private const val REQ_CAMERA = 1

        fun isPairLink(text: String): Boolean = text.trim().startsWith("rustybee://pair?")
    }
}
