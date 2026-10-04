package sh.zeron.android.voice

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Color
import androidx.core.content.FileProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class VoicePhotoEncoderTest {
    @Test fun nativeCameraWritesPhotoThroughTheCaptureContract() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val folder = File(context.cacheDir, "jarvis-camera").apply { mkdirs() }
        val file = File(folder, "native-camera-test.jpg")
        file.delete()
        val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", file)
        val intent = sh.zeron.android.voice.assistant.JarvisCameraContract().createIntent(context, uri)
        val camera = checkNotNull(intent.resolveActivity(context.packageManager))
        fun shell(command: String) = android.os.ParcelFileDescriptor.AutoCloseInputStream(
            instrumentation.uiAutomation.executeShellCommand(command)
        ).use { it.readBytes() }
        // This test owns a fresh capture task, not a restored camera preview.
        shell("am force-stop ${camera.packageName}")
        instrumentation.uiAutomation.grantRuntimePermission(camera.packageName, android.Manifest.permission.CAMERA)
        fun descendants(root: android.view.accessibility.AccessibilityNodeInfo?): List<android.view.accessibility.AccessibilityNodeInfo> =
            if (root == null) emptyList() else listOf(root) + (0 until root.childCount).flatMap { descendants(root.getChild(it)) }
        fun tap(label: String) {
            val until = System.currentTimeMillis() + 10_000
            var retryAt = System.currentTimeMillis() + 1500
            while (System.currentTimeMillis() < until) {
                val node = descendants(instrumentation.uiAutomation.rootInActiveWindow).firstOrNull {
                    it.isEnabled && (it.contentDescription?.toString() == label || it.text?.toString() == label)
                }
                if (node != null) {
                    val bounds = android.graphics.Rect().also { node.getBoundsInScreen(it) }
                    // Dispatch through Android's input service, just as the
                    // manual native-camera check does, after preview startup.
                    Thread.sleep(700)
                    android.os.ParcelFileDescriptor.AutoCloseInputStream(instrumentation.uiAutomation.executeShellCommand(
                        "input tap ${bounds.centerX()} ${bounds.centerY()}"
                    )).use { it.readBytes() }
                    return
                }
                // The emulator camera publishes its shutter before its cold
                // preview is ready. Retry only while that control is present;
                // a completed capture replaces it with the review controls.
                if (label == "Done" && System.currentTimeMillis() >= retryAt && descendants(instrumentation.uiAutomation.rootInActiveWindow).any {
                    it.isEnabled && it.contentDescription?.toString() == "Shutter"
                }) { tap("Shutter"); retryAt = System.currentTimeMillis() + 1500 }
                Thread.sleep(50)
            }
            error("Camera control missing: $label")
        }
        try {
            instrumentation.runOnMainSync { context.startActivity(intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)) }
            tap("Shutter")
            tap("Done")
            val until = System.currentTimeMillis() + 5_000
            while (file.length() == 0L && System.currentTimeMillis() < until) Thread.sleep(50)
            assertTrue("System camera must write the full image to our private URI", file.length() > 0)
            val jpeg = VoicePhotoEncoder.encode(file, 48_000)
            assertTrue(jpeg.size <= 48_000)
            val photo = checkNotNull(BitmapFactory.decodeByteArray(jpeg, 0, jpeg.size))
            assertTrue(photo.width > 0 && photo.height > 0)
            photo.recycle()
        } finally {
            context.revokeUriPermission(uri, android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION or android.content.Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
            file.delete()
            shell("input keyevent 4")
        }
    }

    @Test fun photoIsDownscaledEncodedAndLimitedToPrivateCameraUri() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val folder = File(context.cacheDir, "jarvis-camera").apply { mkdirs() }
        val file = File(folder, "encoding-test.jpg")
        val bitmap = Bitmap.createBitmap(2048, 1536, Bitmap.Config.ARGB_8888)
        try {
            val random = java.util.Random(123)
            val pixels = IntArray(bitmap.width * bitmap.height) { Color.rgb(random.nextInt(256), random.nextInt(256), random.nextInt(256)) }
            bitmap.setPixels(pixels, 0, bitmap.width, 0, 0, bitmap.width, bitmap.height)
            file.outputStream().use { bitmap.compress(Bitmap.CompressFormat.JPEG, 95, it) }
            val jpeg = VoicePhotoEncoder.encode(file, 48_000)
            assertTrue(jpeg.size <= 48_000)
            val options = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeByteArray(jpeg, 0, jpeg.size, options)
            assertEquals("image/jpeg", options.outMimeType)
            assertTrue(options.outWidth in 320..1280)
            val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", file)
            assertEquals("content", uri.scheme)
            assertArrayEquals(file.readBytes(), context.contentResolver.openInputStream(uri)!!.use { it.readBytes() })
            assertTrue(runCatching { FileProvider.getUriForFile(context, "${context.packageName}.files", File(context.cacheDir, "private-data.jpg")) }.isFailure)
        } finally { bitmap.recycle(); file.delete() }
    }
}
