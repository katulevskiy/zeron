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
