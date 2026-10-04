package sh.zeron.android.voice

import androidx.test.ext.junit.runners.AndroidJUnit4
import kotlinx.coroutines.async
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.yield
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.util.Base64

/** Runs against Android's actual JSON serializer, not the JVM test dependency. */
@RunWith(AndroidJUnit4::class)
class VoicePhotoChannelAndroidTest {
    @Test fun imageAtNegotiatedLimitFitsEvenWithSlashHeavyBase64() = runBlocking {
        val limit = 65_536
        val photo = ByteArray((limit - 1024) / 4 * 3) { 0xff.toByte() }
        var sent: ByteArray? = null
        val channel = VoicePhotoChannel { sent = it; true }
        try {
            val result = async { channel.add(photo, limit) }
            yield()
            val wire = checkNotNull(sent) { "An image within the negotiated byte budget must be sent" }
            assertTrue(wire.size <= limit)
            val item = JSONObject(wire.toString(Charsets.UTF_8)).getJSONObject("item")
            val image = item.getJSONArray("content").getJSONObject(0).getString("image_url")
            assertArrayEquals(photo, Base64.getDecoder().decode(image.substringAfter(',')))
            channel.receive(JSONObject().put("type", "conversation.item.added").put("item", JSONObject().put("id", item.getString("id"))).toString().toByteArray())
            result.await()
        } finally { channel.close() }
    }
}
