package sh.zeron.android.voice

import android.graphics.Bitmap
import android.graphics.ImageDecoder
import androidx.core.graphics.scale
import java.io.ByteArrayOutputStream
import java.io.File
import kotlin.math.max
import kotlin.math.roundToInt

/** EXIF-aware, bounded decoding. Re-encoding also drops location/camera metadata.
 * Images fit the negotiated SCTP message size rather than filling an unbounded
 * data-channel queue. Call off the UI thread. */
internal object VoicePhotoEncoder {
    fun encode(file: File, maxBytes: Int): ByteArray {
        require(file.length() in 1..32L * 1024 * 1024) { "Couldn't read the camera photo." }
        require(maxBytes >= 8_000)
        var bitmap = ImageDecoder.decodeBitmap(ImageDecoder.createSource(file)) { decoder, info, _ ->
            val scale = minOf(1f, 1280f / max(info.size.width, info.size.height))
            decoder.setTargetSize(maxOf(1, (info.size.width * scale).roundToInt()), maxOf(1, (info.size.height * scale).roundToInt()))
            decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
        }
        try {
            while (true) {
                for (quality in listOf(86, 72, 58, 44)) {
                    val output = ByteArrayOutputStream()
                    check(bitmap.compress(Bitmap.CompressFormat.JPEG, quality, output))
                    if (output.size() <= maxBytes) return output.toByteArray()
                }
                require(max(bitmap.width, bitmap.height) > 320) { "The photo is too large for this connection." }
                val smaller = bitmap.scale(maxOf(1, (bitmap.width * .75f).roundToInt()), maxOf(1, (bitmap.height * .75f).roundToInt()), true)
                if (smaller !== bitmap) bitmap.recycle()
                bitmap = smaller
            }
        } finally { bitmap.recycle() }
    }
}
