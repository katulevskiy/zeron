package sh.zeron.android.voice

import kotlinx.coroutines.*
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.util.Base64

class VoicePhotoChannelTest {
    @Test fun photoNeedsItsOwnServerAcknowledgementAndUsesImageContent() = runBlocking {
        var event: JSONObject? = null
        val channel = VoicePhotoChannel { event = JSONObject(it.toString(Charsets.UTF_8)); true }
        val photo = byteArrayOf(1, 2, 3)
        val job = async { channel.add(photo, 65_536) }
        yield()
        val item = event!!.getJSONObject("item")
        assertTrue(item.getString("id").length <= 32)
        assertEquals("message", item.getString("type"))
        assertEquals("user", item.getString("role"))
        val content = item.getJSONArray("content").getJSONObject(0)
        assertEquals("input_image", content.getString("type"))
        assertArrayEquals(photo, Base64.getDecoder().decode(content.getString("image_url").substringAfter(',')))
        channel.receive("{\"type\":\"conversation.item.added\",\"item\":{\"id\":\"other\"}}".toByteArray())
        assertFalse(job.isCompleted)
        channel.receive(JSONObject().put("type", "conversation.item.added").put("item", item).toString().toByteArray())
        job.await()
        channel.close()
    }

    @Test fun providerRejectionAndCloseCannotReportSuccessOrExposePayloads() = runBlocking {
        var event: JSONObject? = null
        val channel = VoicePhotoChannel { event = JSONObject(it.toString(Charsets.UTF_8)); true }
        val result = async { runCatching { channel.add(byteArrayOf(1), 65_536) } }
        yield()
        channel.receive(JSONObject().put("type", "error").put("error", JSONObject()
            .put("event_id", event!!.getString("event_id")).put("message", "PRIVATE_IMAGE_DATA")).toString().toByteArray())
        assertFalse(result.await().isSuccess)
        assertEquals(VoicePhotoException.Reason.REJECTED, (result.await().exceptionOrNull() as VoicePhotoException).reason)
        assertFalse(result.await().exceptionOrNull()!!.message!!.contains("PRIVATE_IMAGE_DATA"))
        val second = async { runCatching { channel.add(byteArrayOf(2), 65_536) } }
        yield()
        channel.close()
        assertFalse(second.await().isSuccess)
        assertEquals(VoicePhotoException.Reason.ENDED, (second.await().exceptionOrNull() as VoicePhotoException).reason)
        assertTrue(runCatching { channel.add(byteArrayOf(3), 65_536) }.isFailure)
    }

    @Test fun oversizedAndRefusedSendsDoNotWaitForServerOrLeaveBusySlot() = runBlocking {
        var sends = 0
        val channel = VoicePhotoChannel { sends++; false }
        assertEquals(VoicePhotoException.Reason.SIZE, (runCatching { channel.add(ByteArray(50_000), 65_536) }.exceptionOrNull() as VoicePhotoException).reason)
        assertEquals(0, sends)
        assertEquals(VoicePhotoException.Reason.TRANSPORT, (runCatching { channel.add(byteArrayOf(1), 65_536) }.exceptionOrNull() as VoicePhotoException).reason)
        assertTrue(runCatching { channel.add(byteArrayOf(2), 65_536) }.isFailure)
        assertEquals(2, sends)
    }
}
