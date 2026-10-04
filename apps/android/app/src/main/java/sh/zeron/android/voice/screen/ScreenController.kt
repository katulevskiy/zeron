package sh.zeron.android.voice.screen

import android.app.Application
import android.app.KeyguardManager
import android.graphics.Bitmap
import android.os.SystemClock
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import org.json.JSONObject
import sh.zeron.android.core.AppModel
import sh.zeron.android.voice.assistant.JarvisAssistantSession
import java.io.File
import java.util.UUID

data class PhoneScreenState(val controlling: Boolean = false, val sharing: Boolean = false, val message: String? = null)

/** Call-owned capabilities. Persistent settings do not start capture or input. */
class ScreenController(private val app: Application, private val model: AppModel) {
    val settings = ScreenSettings(app)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val _state = MutableStateFlow(PhoneScreenState())
    val state = _state.asStateFlow()
    private val jev = JevClient(settings::readKey)
    private var bridge: PhoneControlBridge? = null
    private var generation: Long? = null
    private var streaming: Job? = null
    private var shareJob: Job? = null
    private var experiment: Job? = null
    private var assist: Bitmap? = null
    private var assistAt = 0L
    private var latestFrame: Bitmap? = null
    private var latestFrameAt = 0L
    private var folder: File? = null
    private var imageNumber = 0
    init {
        androidx.core.content.ContextCompat.registerReceiver(app, object : android.content.BroadcastReceiver() {
            override fun onReceive(context: android.content.Context?, intent: android.content.Intent?) { stop() }
        }, android.content.IntentFilter(android.content.Intent.ACTION_SCREEN_OFF), androidx.core.content.ContextCompat.RECEIVER_NOT_EXPORTED)
        scope.launch { model.jarvis.state.collect { if (!it.live) stop() } }
        scope.launch { settings.contextEnabled.collect {
            sh.zeron.android.voice.assistant.JarvisAssistantService.refreshContext()
            if (!it) { stopSharing(); clearAssist() }
        } }
        scope.launch { settings.controlEnabled.collect { if (!it) stopControl() } }
    }
    fun setContext(enabled: Boolean) { settings.setContext(enabled) }
    fun setControl(enabled: Boolean) { settings.setControl(enabled) }
    fun checkKey() { scope.launch {
        try { _state.value = _state.value.copy(message = "Jev connected · ${jev.check()} ms") }
        catch (e: CancellationException) { throw e }
        catch (_: Exception) { _state.value = _state.value.copy(message = "Couldn't verify Jev key. Check the key and network.") }
    } }
    internal fun acceptAssist(bitmap: Bitmap?) {
        clearAssistImage()
        if (bitmap != null && settings.contextEnabled.value && !locked()) {
            assist = bitmap.copy(Bitmap.Config.ARGB_8888, false); assistAt = SystemClock.elapsedRealtime()
        }
    }
    private fun clearAssistImage() { assist?.recycle(); assist = null; assistAt = 0 }
    private fun clearAssist() { clearAssistImage() }
    private fun locked() = app.getSystemService(KeyguardManager::class.java).isKeyguardLocked
    private fun currentCall(): Long {
        check(!locked()) { "Unlock your phone" }
        return model.jarvis.currentCallId?.takeIf { model.jarvis.state.value.call?.phase == uniffi.zeron_core.VoiceCallPhase.ACTIVE }
            ?: error("Start a Jarvis call first")
    }
    private fun accessibility() = JarvisAccessibilityService.current ?: error("Enable Jarvis phone control in Android Accessibility settings")
    private fun requireControl() {
        check(settings.controlEnabled.value && _state.value.controlling && generation == currentCall()) { "Phone control is stopped" }
    }
    fun startControl() {
        if (bridge != null) return
        scope.launch {
            try {
                val id = currentCall()
                check(settings.controlEnabled.value) { "Enable phone control in Jarvis settings" }
                check(model.jarvis.isPhoneHost) { "Choose This phone as Jarvis's execution device for phone control" }
                accessibility(); check(settings.readKey() != null) { "Add your Jev key in Jarvis settings" }
                generation = id
                val server = PhoneControlBridge(scope, ::allowed, ::handle)
                bridge = server
                _state.value = _state.value.copy(controlling = true, message = "Phone control ready")
                try { model.jarvis.sendContext(id, server.instructions()) }
                catch (e: Exception) { stopControl(); throw e }
            } catch (e: CancellationException) { throw e }
            catch (e: Exception) { _state.value = _state.value.copy(message = e.message ?: "Couldn't start phone control") }
        }
    }
    internal fun allowed() = settings.controlEnabled.value && _state.value.controlling &&
        generation != null && model.jarvis.owns(generation!!) && !locked()
    internal suspend fun handle(command: JSONObject): JSONObject {
        requireControl()
        val service = accessibility()
        // Assistant window must not intercept synthetic touches or cover the
        // app being read. The established voice call and microphone continue.
        JarvisAssistantSession.hideForControl()
        yield()
        return when (command.getString("operation")) {
            "state" -> service.observe().json()
            "screenshot" -> {
                check(settings.contextEnabled.value) { "Enable screen context in Jarvis settings" }
                val before = service.observe()
                val bitmap = service.screenshot()
                try {
                    requireControl(); ScreenPolicy.requireFresh(before, service.observe(), SystemClock.elapsedRealtime())
                    saveForAgent(bitmap, before)
                } finally { bitmap.recycle() }
            }
            "tap", "swipe" -> {
                val screen = service.observe()
                // The caller must bind coordinates to a fresh observation.
                require(command.getString("fingerprint") == screen.fingerprint) { "Screen changed; observe again" }
                if (command.getString("operation") == "tap") service.tap(screen, command.getDouble("x"), command.getDouble("y"))
                else service.swipe(screen, command.getDouble("x"), command.getDouble("y"), command.getDouble("end_x"), command.getDouble("end_y"))
                requireControl()
                JSONObject().put("status", "executed").put("screen", service.observe().json())
            }
            "act" -> act(command)
            "stop" -> { stopControl(); JSONObject().put("status", "stopped") }
            else -> error("Unsupported operation")
        }
    }
    private suspend fun act(command: JSONObject): JSONObject {
        val goal = command.getString("goal")
        val supplied = command.optString("text").takeIf { it.isNotEmpty() }
        require(supplied == null || supplied.length <= 2000)
        val steps = command.optInt("max_steps", 1); require(steps in 1..8)
        val history = mutableListOf<String>()
        val trace = org.json.JSONArray()
        var unchanged = 0
        repeat(steps) {
            requireControl()
            val screen = accessibility().observe()
            val decision = jev.decide(screen, goal, supplied, history)
            requireControl()
            trace.put(JSONObject().put("operation", decision.operation).put("target", decision.target)
                .put("confidence", decision.confidence).put("decision_ms", decision.latencyMs))
            when (decision.operation) {
                "DONE" -> return JSONObject().put("status", "done_unverified").put("steps", trace).put("screen", accessibility().observe().json())
                "BLOCKED" -> return JSONObject().put("status", "blocked").put("steps", trace).put("screen", screen.json())
                "WAIT" -> delay(200)
                else -> try { accessibility().execute(screen, decision, supplied) }
                catch (_: StaleScreenException) {
                    trace.getJSONObject(trace.length() - 1).put("status", "stale_no_input")
                    history.add("Screen changed before action; no input executed")
                    delay(25)
                    return@repeat
                }
            }
            history.add("${decision.operation} ${decision.target.orEmpty()}")
            // Wait for useful observable changes, capped at 250 ms. No blind
            // second-long sleep between steps and no unbounded autonomous loop.
            var after = accessibility().observe()
            if (after.fingerprint == screen.fingerprint) {
                val deadline = SystemClock.elapsedRealtime() + 250
                while (after.fingerprint == screen.fingerprint && SystemClock.elapsedRealtime() < deadline) {
                    delay(25); requireControl(); after = accessibility().observe()
                }
            }
            unchanged = if (after.fingerprint == screen.fingerprint) unchanged + 1 else 0
            if (unchanged >= 2) return JSONObject().put("status", "stalled").put("steps", trace).put("screen", after.json())
        }
        return JSONObject().put("status", "step_limit").put("steps", trace).put("screen", accessibility().observe().json())
    }
    /** On-phone lab UI uses the very same handler as GPT's bridge. */
    fun runGoal(goal: String, text: String?) {
        if (experiment?.isActive == true) return
        experiment = scope.launch {
            try {
                delay(350) // let the explicitly dismissed settings task return to the previous app
                val result = handle(JSONObject().put("operation", "act").put("goal", goal).put("text", text.orEmpty()).put("max_steps", 8))
                _state.value = _state.value.copy(message = "${result.getString("status")} · ${result.getJSONArray("steps").length()} decisions")
            } catch (e: CancellationException) { throw e }
            catch (e: Exception) { _state.value = _state.value.copy(message = e.message ?: "Phone control failed") }
        }
    }
    private suspend fun saveForAgent(bitmap: Bitmap, screen: ScreenSnapshot): JSONObject = withContext(Dispatchers.IO) {
        val dir = folder ?: model.phone.paths.host("/tmp/jarvis-screen-${UUID.randomUUID()}")!!.also { it.mkdirs(); folder = it }
        val file = File(dir, "screen-${++imageNumber}.jpg")
        file.outputStream().use { bitmap.compress(Bitmap.CompressFormat.JPEG, 85, it) }
        val bytes = sh.zeron.android.voice.VoicePhotoEncoder.encode(file, 384_000)
        file.writeBytes(bytes)
        dir.listFiles()?.sortedByDescending { it.lastModified() }?.drop(5)?.forEach { it.delete() }
        val size = android.graphics.BitmapFactory.Options().apply { inJustDecodeBounds = true }
        android.graphics.BitmapFactory.decodeFile(file.path, size)
        val origin = accessibility().let { service -> withContext(Dispatchers.Main.immediate) { service.imageBounds() } }
        screen.json().put("image_path", model.phone.paths.guest(file)).put("image_width", size.outWidth).put("image_height", size.outHeight)
            .put("image_bounds_in_screen", org.json.JSONArray(origin))
    }
    fun shareOnce() {
        if (shareJob?.isActive == true) return
        shareJob = scope.launch {
            try {
                val id = currentCall(); check(settings.contextEnabled.value) { "Enable screen context in Jarvis settings" }
                _state.value = _state.value.copy(message = "Sharing screen…")
                val bitmap = capture()
                val receipt = try { deliver(id, bitmap) } finally { bitmap.recycle() }
                _state.value = _state.value.copy(message = receipt.replace("Photo", "Screen"))
            } catch (e: CancellationException) { throw e }
            catch (e: Exception) { _state.value = _state.value.copy(message = e.message ?: "Couldn't capture screen") }
        }
    }
    private suspend fun capture(): Bitmap {
        check(settings.contextEnabled.value && !locked()) { "Screen context is stopped" }
        if (_state.value.sharing && model.jarvis.assistantVisible) { JarvisAssistantSession.hideForControl(); delay(300) }
        if (_state.value.sharing) synchronized(this) {
            latestFrame?.takeIf { SystemClock.elapsedRealtime() - latestFrameAt < 3000 }?.let { return it.copy(Bitmap.Config.ARGB_8888, false) }
        }
        assist?.takeIf { SystemClock.elapsedRealtime() - assistAt < 10_000 }?.let {
            val copy = it.copy(Bitmap.Config.ARGB_8888, false); clearAssistImage(); return copy
        }
        if (android.os.Build.VERSION.SDK_INT < 34) { JarvisAssistantSession.hideForControl(); delay(100) }
        return accessibility().screenshot()
    }
    private suspend fun deliver(id: Long, bitmap: Bitmap): String {
        val file = withContext(Dispatchers.IO) { File.createTempFile("jarvis-screen-", ".jpg", app.cacheDir).also { target ->
            target.outputStream().use { bitmap.compress(Bitmap.CompressFormat.JPEG, 85, it) }
        } }
        return try { model.jarvis.addPhoto(id, file, screenImage = true) { model.jarvis.owns(id) && settings.contextEnabled.value && !locked() } }
        finally { file.delete() }
    }
    fun launchProjection(context: android.content.Context) {
        try {
            currentCall(); check(settings.contextEnabled.value) { "Enable screen context in Jarvis settings" }
            JarvisAssistantSession.hideForControl()
            context.startActivity(android.content.Intent(context, ScreenProjectionActivity::class.java).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK))
        } catch (e: Exception) { _state.value = _state.value.copy(message = e.message) }
    }
    internal fun projectionStarted(id: Long): Boolean {
        if (!model.jarvis.owns(id) || !settings.contextEnabled.value || locked()) return false
        generation = id
        _state.value = _state.value.copy(sharing = true, message = "Sharing screen with Jarvis")
        streaming?.cancel()
        streaming = scope.launch {
            var signature: Int? = null
            while (model.jarvis.owns(id) && settings.contextEnabled.value && !locked()) {
                delay(2000)
                if (model.jarvis.assistantVisible || !model.jarvis.canStreamScreen(id)) continue
                val bitmap = synchronized(this@ScreenController) { latestFrame?.copy(Bitmap.Config.ARGB_8888, false) } ?: continue
                try {
                    val small = Bitmap.createScaledBitmap(bitmap, 24, 24, false)
                    val pixels = IntArray(24 * 24); small.getPixels(pixels, 0, 24, 0, 0, 24, 24); if (small !== bitmap) small.recycle()
                    val hash = pixels.map { it and 0x00F0F0F0 }.hashCode()
                    if (hash != signature) { deliver(id, bitmap); signature = hash }
                } catch (e: CancellationException) { throw e }
                catch (_: Exception) { _state.value = _state.value.copy(message = "Screen delivery failed; sharing paused"); break }
                finally { bitmap.recycle() }
            }
            stopSharing()
        }
        return true
    }
    internal fun frame(bitmap: Bitmap) = synchronized(this) {
        if (!_state.value.sharing || locked() || model.jarvis.assistantVisible) { bitmap.recycle(); return@synchronized }
        latestFrame?.recycle(); latestFrame = bitmap; latestFrameAt = SystemClock.elapsedRealtime()
    }
    internal fun report(message: String) { _state.value = _state.value.copy(message = message) }
    internal fun projectionEnded() {
        streaming?.cancel(); streaming = null
        synchronized(this) { latestFrame?.recycle(); latestFrame = null }
        _state.value = _state.value.copy(sharing = false)
    }
    fun stopSharing() {
        shareJob?.cancel(); shareJob = null
        projectionEnded(); app.stopService(android.content.Intent(app, ScreenProjectionService::class.java))
    }
    fun stopControl() {
        experiment?.cancel(); experiment = null
        bridge?.close(); bridge = null
        folder?.deleteRecursively(); folder = null
        _state.value = _state.value.copy(controlling = false)
    }
    fun stop() { stopControl(); stopSharing(); clearAssist(); generation = null }
}
