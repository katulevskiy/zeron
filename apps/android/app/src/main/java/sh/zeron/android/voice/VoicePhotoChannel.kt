package sh.zeron.android.voice

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withTimeout
import org.json.JSONArray
import org.json.JSONObject
import java.util.Base64
import java.util.UUID
import java.util.concurrent.atomic.AtomicReference

/** One bounded photo request on the existing authenticated WebRTC channel.
 * A successful send is NOT an acknowledgement that the model received it. */
internal class VoicePhotoChannel(private val send: (ByteArray) -> Boolean) {
    private class Pending(val eventId: String, val itemId: String, val result: CompletableDeferred<Unit>)
    private val pending = AtomicReference<Pending?>(null)
    @Volatile private var closed = false

    suspend fun add(jpeg: ByteArray, maxMessageBytes: Int) {
        if (closed) throw VoicePhotoException(VoicePhotoException.Reason.ENDED)
        if (jpeg.isEmpty()) throw VoicePhotoException(VoicePhotoException.Reason.READ)
        val id = UUID.randomUUID().toString().replace("-", "")
        val request = Pending("photo_event_$id", "photo_${id.take(24)}", CompletableDeferred())
        if (!pending.compareAndSet(null, request)) throw VoicePhotoException(VoicePhotoException.Reason.BUSY)
        try {
            val event = JSONObject().put("type", "conversation.item.create").put("event_id", request.eventId)
                .put("item", JSONObject().put("id", request.itemId).put("type", "message").put("role", "user")
                    .put("content", JSONArray().put(JSONObject().put("type", "input_image")
                        .put("image_url", "data:image/jpeg;base64," + Base64.getEncoder().encodeToString(jpeg)))))
            // Android's JSONStringer escapes every '/'; the JVM org.json does
            // not. Base64 can contain many slashes, inflating a correctly sized
            // photo beyond SCTP's limit. JSON permits literal forward slashes.
            // All strings here are protocol constants, UUIDs, or Base64, so
            // removing this optional escape preserves the exact image bytes.
            val data = event.toString().replace("\\/", "/").toByteArray(Charsets.UTF_8)
            if (data.size > maxMessageBytes) throw VoicePhotoException(VoicePhotoException.Reason.SIZE)
            if (closed) throw VoicePhotoException(VoicePhotoException.Reason.ENDED)
            if (!send(data)) throw VoicePhotoException(VoicePhotoException.Reason.TRANSPORT)
            withTimeout(10_000) { request.result.await() }
        } finally { pending.compareAndSet(request, null) }
    }

    /** Only match our own item/event ids. Do not consume captions/tool events or
     * expose provider error payloads (which can contain private request data). */
    fun receive(data: ByteArray) {
        if (closed || data.size > 262_144) return
        val request = pending.get() ?: return
        val event = runCatching { JSONObject(data.toString(Charsets.UTF_8)) }.getOrNull() ?: return
        when (event.optString("type")) {
            "conversation.item.added", "conversation.item.created" -> {
                if (event.optJSONObject("item")?.optString("id") == request.itemId) request.result.complete(Unit)
            }
            "error" -> if (event.optJSONObject("error")?.optString("event_id") == request.eventId) {
                request.result.completeExceptionally(VoicePhotoException(VoicePhotoException.Reason.REJECTED))
            }
        }
    }

    fun close() {
        closed = true
        pending.getAndSet(null)?.result?.completeExceptionally(VoicePhotoException(VoicePhotoException.Reason.ENDED))
    }
}
