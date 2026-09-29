package sh.zeron.android.core

import android.content.Context
import android.graphics.Bitmap
import android.graphics.ImageDecoder
import android.net.Uri
import android.util.Log
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
import java.util.UUID

/**
 * Photo attachments: picked images are decoded downsampled (long side ≤ 2048
 * px — agents read screenshots fine at that size and relay uploads stay
 * small), re-encoded as JPEG, and given a square-ish thumbnail for the
 * composer strip. Runs off the main thread.
 */
object Attachments {
    const val MAX_IMAGES = 8
    private const val MAX_SIDE = 2048
    private const val THUMB_SIDE = 168

    suspend fun stage(context: Context, uris: List<Uri>): List<StagedImage> = withContext(Dispatchers.IO) {
        uris.mapIndexedNotNull { i, uri ->
            runCatching { stageOne(context, uri, i) }.onFailure { Log.w(AppModel.TAG, "attachment $uri", it) }.getOrNull()
        }
    }

    private fun stageOne(context: Context, uri: Uri, index: Int): StagedImage {
        val source = ImageDecoder.createSource(context.contentResolver, uri)
        val bitmap = ImageDecoder.decodeBitmap(source) { decoder, info, _ ->
            val (w, h) = info.size.width to info.size.height
            val scale = minOf(1f, MAX_SIDE.toFloat() / maxOf(w, h))
            if (scale < 1f) decoder.setTargetSize((w * scale).toInt().coerceAtLeast(1), (h * scale).toInt().coerceAtLeast(1))
            // Software bitmaps: hardware ones can't be compressed or scaled.
            decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
        }
        val bytes = ByteArrayOutputStream().use { out ->
            bitmap.compress(Bitmap.CompressFormat.JPEG, 86, out)
            out.toByteArray()
        }
        val k = THUMB_SIDE.toFloat() / minOf(bitmap.width, bitmap.height)
        val thumb = if (k < 1f) Bitmap.createScaledBitmap(bitmap, (bitmap.width * k).toInt().coerceAtLeast(1), (bitmap.height * k).toInt().coerceAtLeast(1), true) else bitmap
        val stamp = System.currentTimeMillis() / 1000
        return StagedImage(UUID.randomUUID().toString(), "photo-$stamp-${index + 1}.jpg", bytes, thumb)
    }
}
