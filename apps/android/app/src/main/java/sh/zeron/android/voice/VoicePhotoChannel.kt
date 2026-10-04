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
        check(!closed) { "The call ended before the photo was added." }
        require(jpeg.isNotEmpty()) { "The photo is empty." }
        val id = UUID.randomUUID().toString().replace("-", "")
        val request = Pending("photo_event_$id", "photo_${id.take(24)}", CompletableDeferred())
        check(pending.compareAndSet(null, request)) { "A photo is already being added." }
        try {
            val event = JSONObject().put("type", "conversation.item.create").put("event_id", request.eventId)
                .put("item", JSONObject().put("id", request.itemId).put("type", "message").put("role", "user")
                    .put("content", JSONArray().put(JSONObject().put("type", "input_image")
                        .put("image_url", "data:image/jpeg;base64," + Base64.getEncoder().encodeToString(jpeg)))))
            val data = event.toString().toByteArray(Charsets.UTF_8)
            require(data.size <= maxMessageBytes) { "The photo is too large for this connection." }
            check(!closed && send(data)) { "Couldn't send the photo. Try again." }
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
                request.result.completeExceptionally(IllegalStateException("Jarvis couldn't accept this photo. Try again."))
            }
        }
    }

    fun close() {
        closed = true
        pending.getAndSet(null)?.result?.completeExceptionally(IllegalStateException("The call ended before the photo was added."))
    }
}
