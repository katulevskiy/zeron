package sh.zeron.android.core

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.util.Log
import android.util.LruCache
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.zeron_core.BusyPolicy
import uniffi.zeron_core.ChatConfig
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.FileMatch
import uniffi.zeron_core.ModelInfo
import uniffi.zeron_core.QueueEditAction
import uniffi.zeron_core.QueueEditLease
import uniffi.zeron_core.QueueEditStart
import uniffi.zeron_core.SandboxLevel
import uniffi.zeron_core.SendRequest
import uniffi.zeron_core.SessionHandle
import uniffi.zeron_core.TranscriptView
import uniffi.zeron_core.UserInputAnswer
import uniffi.zeron_core.fallbackModels
import uniffi.zeron_core.reasoningLabel
import java.util.UUID

/** How a message sent during a running turn is delivered. */
enum class DeliveryMode { Queue, Steer, Interrupt }

enum class QueueAction { SendNow, Edit, MoveUp, MoveDown, Remove }

/**
 * One open session: maps the core's composer state to [SessionChrome] and
 * carries every command. The transcript itself flows Rust→Rust
 * (`TranscriptView.attach`) and never crosses here as rows.
 */
class SessionController(private val app: AppModel, private val client: CoreClient, val chatId: String) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val handle: SessionHandle? = runCatching { client.openSession(chatId) }.onFailure { Log.w(AppModel.TAG, "open session", it) }.getOrNull()

    private val _chrome = MutableStateFlow(SessionChrome())
    val chrome: StateFlow<SessionChrome> = _chrome.asStateFlow()

    val available get() = handle != null
    private var sendFailure: String? = null

    private val labels = object : SessionChrome.Labels {
        override fun reasoning(level: String) = reasoningLabel(level)
    }

    init {
        refresh()
        scope.launch { app.sessionPulse.collect { if (it == chatId) refresh() } }
        scope.launch { app.workspace.collect { refresh() } }
        scope.launch { app.connectivity.collect { refresh() } }
        // Reconnect countdowns tick without events.
        scope.launch {
            while (true) {
                delay(1000)
                if (_chrome.value.banner is Banner.Reconnecting) refresh()
            }
        }
    }

    fun refresh() {
        val h = handle ?: return
        val c = runCatching { h.composer() }.getOrNull() ?: return
        _chrome.value = SessionChrome.from(c, app.row(chatId), app.connectivity.value, sendFailure, System.currentTimeMillis(), labels)
    }

    fun attach(engine: TranscriptView) {
        engine.attach(client, chatId)
        handle?.setViewAttached(true)
        app.markSeen(chatId)
    }

    fun detach() {
        handle?.setViewAttached(false)
        app.markSeen(chatId)
    }

    fun reattach() = handle?.setViewAttached(true)

    /** False when the core refused the message (it stays in the composer). */
    fun send(text: String, images: List<StagedImage>, mode: DeliveryMode): Boolean {
        val h = handle ?: return false
        return try {
            if (mode == DeliveryMode.Interrupt && _chrome.value.running) h.interrupt()
            h.send(SendRequest(text, images.map { it.outgoing }, null, if (mode == DeliveryMode.Steer) BusyPolicy.STEER else BusyPolicy.QUEUE))
            sendFailure = null
            refresh()
            true
        } catch (e: Exception) {
            sendFailure = "Couldn't send: ${e.userMessage()}"
            refresh()
            false
        }
    }

    fun stop() {
        runCatching { handle?.interrupt() }
    }

    fun answer(requestId: String, answers: List<Pair<String, List<String>>>) {
        runCatching { handle?.respondInput(requestId, answers.map { UserInputAnswer(it.first, it.second) }) }
            .onFailure { Log.w(AppModel.TAG, "respond input", it) }
    }

    fun queueAction(id: String, action: QueueAction) {
        val h = handle ?: return
        when (action) {
            // Steers text into the live turn (never interrupts); attachment rows send now.
            QueueAction.SendNow -> scope.launch { runCatching { h.deliverQueuedNow(id) } }
            QueueAction.Remove -> scope.launch { runCatching { h.removeQueued(id) } }
            QueueAction.MoveUp -> runCatching { h.moveQueuedBy(id, -1) }
            QueueAction.MoveDown -> runCatching { h.moveQueuedBy(id, 1) }
            QueueAction.Edit -> Unit // driven by the screen: beginEdit / finishEdit
        }
    }

    fun retryDelivery() {
        runCatching { handle?.retryDelivery() }
    }

    fun clearQueueError() = handle?.clearQueueError()

    // ── queued-message edits (host lease) ─────────────────────────────────

    private var lease: QueueEditLease? = null
    private var renewal: Job? = null
    private var editBase: Pair<String, String>? = null

    /** Lease a queued message for editing; its visible text when acquired. */
    suspend fun beginEdit(id: String): String? {
        val h = handle ?: return null
        val start = h.beginQueuedEdit(id, UUID.randomUUID().toString())
        val acquired = start as? QueueEditStart.Acquired ?: return null
        lease = acquired.lease
        renewal?.cancel()
        renewal = scope.launch {
            while (true) {
                delay(20_000)
                val l = lease ?: return@launch
                if (!h.renewQueuedEdit(l)) return@launch
            }
        }
        val item = h.composer().queue.firstOrNull { it.id == id } ?: return null
        editBase = item.text to item.visibleText
        return item.visibleText
    }

    /** Commit (text) or cancel (null) the active edit. */
    suspend fun finishEdit(text: String?) {
        renewal?.cancel()
        val base = editBase
        editBase = null
        val l = lease ?: return
        lease = null
        var body = text
        if (text != null && base != null) {
            // Unchanged: release the row as it was (a commit would drop what the composer never showed).
            body = if (text == base.second) null else QueueText.replacingVisible(base.first, base.second, text)
        }
        handle?.finishQueuedEdit(l, if (body == null) QueueEditAction.CANCEL else QueueEditAction.COMMIT, body)
    }

    // ── config chips ──────────────────────────────────────────────────────

    suspend fun models(): List<ModelInfo> {
        val harness = app.row(chatId)?.harness ?: "claude-code"
        val device = _chrome.value.hostDevice
        return runCatching { client.listModels(device, harness) }.getOrElse { fallbackModels(harness) }
    }

    fun setConfig(change: (ChatConfig) -> ChatConfig) {
        val current = client.sessionConfig(chatId)
            ?: ChatConfig(app.row(chatId)?.harness ?: "claude-code", null, null, emptyMap(), SandboxLevel.WORKSPACE_WRITE)
        runCatching { client.setSessionConfig(chatId, change(current)) }.onFailure { Log.w(AppModel.TAG, "set config", it) }
        app.requestRefresh()
    }

    suspend fun searchFiles(query: String): List<FileMatch> =
        app.searchFiles(_chrome.value.hostDevice, chatId, null, query)

    /** Attachment bytes → a bitmap, cached across sessions. */
    suspend fun loadImage(reference: String): Bitmap? {
        ImageCache.get(reference)?.let { return it }
        val device = _chrome.value.hostDevice
        val bytes = runCatching { client.readAttachment(device, reference) }.getOrNull() ?: return null
        val bmp = withContext(Dispatchers.Default) { ImageCache.decode(bytes, 1600) } ?: return null
        ImageCache.put(reference, bmp)
        return bmp
    }

    fun close() {
        renewal?.cancel()
        detach()
        client.closeSession(chatId)
        scope.cancel()
    }
}

/** Decoded attachment images (memory-bounded). */
object ImageCache {
    private val cache = object : LruCache<String, Bitmap>(48 * 1024 * 1024) {
        override fun sizeOf(key: String, value: Bitmap) = value.allocationByteCount
    }

    fun get(ref: String): Bitmap? = cache.get(ref)
    fun put(ref: String, bmp: Bitmap) {
        cache.put(ref, bmp)
    }

    /** Decode, downsampled so the long side stays near `maxSide`. */
    fun decode(bytes: ByteArray, maxSide: Int): Bitmap? {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
        var sample = 1
        while (maxOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= maxSide) sample *= 2
        return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample })
    }
}
