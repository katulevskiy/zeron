package sh.zeron.android.voice

import android.Manifest
import android.app.Application
import android.content.pm.PackageManager
import androidx.core.content.ContextCompat
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import sh.zeron.android.core.AppModel
import sh.zeron.android.feedback.Cue
import sh.zeron.android.feedback.Haptic
import uniffi.zeron_core.*

data class JarvisState(
    val live: Boolean = false,
    val call: VoiceCallState? = null,
    val muted: Boolean = false,
    val speaker: Boolean = true,
    val host: String = "",
    val activeSince: Long? = null,
    val error: String? = null,
    val endReason: VoiceEndReason? = null,
)

/** Application-owned, single-generation call: navigation and screen locking keep it alive. */
class JarvisController(private val app: Application, private val model: AppModel) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val prefs = app.getSharedPreferences("jarvis", 0)
    private val generation = VoiceGeneration()
    private var client: CoreClient? = null
    private var identity = ""
    private var call: VoiceCall? = null
    private var media: AndroidVoiceMedia? = null
    private var permission: Pair<Long, CompletableDeferred<Boolean>>? = null
    private val _permissionRequest = MutableStateFlow<Long?>(null)
    val permissionRequest = _permissionRequest.asStateFlow()
    private val _consentRequest = MutableStateFlow<VoiceHost?>(null)
    val consentRequest = _consentRequest.asStateFlow()
    private val _state = MutableStateFlow(JarvisState())
    val state = _state.asStateFlow()
    val live = state.map { it.live }.distinctUntilChanged().stateIn(scope, SharingStarted.Eagerly, false)
    private val _hosts = MutableStateFlow<List<VoiceHost>>(emptyList())
    val hosts = _hosts.asStateFlow()
    private val _voices = MutableStateFlow<List<String>>(emptyList())
    val voices = _voices.asStateFlow()
    private val _selectedHost = MutableStateFlow<String?>(null)
    val selectedHost = _selectedHost.asStateFlow()
    private val _selectedVoice = MutableStateFlow<String?>(null)
    val selectedVoice = _selectedVoice.asStateFlow()
    private var assistantOwner: Any? = null
    private val _assistantPresentation = MutableStateFlow(false)
    val assistantPresentation = _assistantPresentation.asStateFlow()
    private var requestedFromAssistant = false
    private var assistantCall = false
    val assistantVisible get() = assistantOwner != null
    private val canStart get() = model.foreground || assistantVisible
    var mediaVisible = true

    /** Only the system-bound session holds this visibility lease; role alone
     * never authorizes a background microphone start. Stale sessions cannot
     * revoke the lease of a newer overlay. */
    fun showAssistant(owner: Any) { assistantOwner = owner; _assistantPresentation.value = true; mediaVisible = true }
    fun hideAssistant(owner: Any, keepCall: Boolean) {
        if (assistantOwner !== owner) return
        assistantOwner = null
        _assistantPresentation.value = false
        if (requestedFromAssistant) dismissConsent()
        if (generation.live && !keepCall) stop()
        mediaVisible = model.foreground
    }
    fun assistantDisabled() {
        assistantOwner = null
        _assistantPresentation.value = false
        if (requestedFromAssistant) dismissConsent()
        if (assistantCall) stop()
        mediaVisible = model.foreground
    }
    val currentCallId get() = generation.current
    fun owns(id: Long) = generation.accepts(id)
    fun stopIfOwned(id: Long) { if (owns(id)) stop() }

    fun bind(next: CoreClient?) {
        val key = next?.let { "${it.userId()}:${it.orgId()}" }.orEmpty()
        if (next !== client || key != identity) {
            finish(null)
            _consentRequest.value = null
            client = next
            identity = key
            _selectedHost.value = prefs.getString("$identity.host", null)
            _selectedVoice.value = prefs.getString("$identity.voice", null)
            _voices.value = defaultVoiceStyles()
        }
        refreshHosts()
    }

    fun refreshHosts() {
        _hosts.value = client?.devices()?.filter {
            it.isExecutionHost && it.capabilities.contains(voiceHostCapability())
        }?.map { VoiceHost(it.id, it.name, it.online, it.isSelf) }.orEmpty()
    }

    fun selectHost(id: String?) {
        if (generation.live) return
        _selectedHost.value = id
        prefs.edit().putString("$identity.host", id).apply()
    }

    fun selectVoice(voice: String?) {
        _selectedVoice.value = voice
        prefs.edit().putString("$identity.voice", voice).apply()
    }

    /** The consent precedes both the call and the platform microphone prompt. */
    fun requestStart() = requestStart(false)
    fun requestStartFromAssistant() { if (assistantVisible) requestStart(true) }
    private fun requestStart(assistant: Boolean) {
        if (generation.live) { if (!assistant) model.pendingRoute.value = "jarvis"; return }
        if (!canStart) return
        requestedFromAssistant = assistant
        refreshHosts()
        val host = startHost(_hosts.value, _selectedHost.value)
        if (host == null) {
            _state.value = JarvisState(error = if (_hosts.value.none { it.online })
                "Jarvis needs an online Zeron device with Codex voice. Install and sign in to Codex on this phone, or choose a compatible desktop."
                else "Choose the device Jarvis should run on.")
            if (!assistant) model.pendingRoute.value = "jarvis-settings"
            return
        }
        if (!prefs.getBoolean("$identity.consent", false)) _consentRequest.value = host
        else start(host)
    }

    fun acceptConsent() {
        val host = _consentRequest.value ?: return
        _consentRequest.value = null
        prefs.edit().putBoolean("$identity.consent", true).apply()
        if (!requestedFromAssistant || assistantVisible) start(host)
    }

    fun dismissConsent() { _consentRequest.value = null }
    fun dismissError() { _state.value = _state.value.copy(error = null) }

    private fun start(host: VoiceHost) {
        val client = client ?: return
        if (generation.live || !canStart || !host.online) return
        selectHost(host.id)
        assistantCall = requestedFromAssistant
        val id = generation.begin()
        _state.value = JarvisState(live = true, host = host.name)
        mediaVisible = true
        model.feedback.both(Haptic.Confirm, Cue.Open)
        val endpoint = AndroidVoiceMedia(app, scope, id, { askPermission(id) }, { mediaVisible }, {
            scope.launch { if (generation.accepts(id)) finish(VoiceEndReason.AUDIO_UNAVAILABLE) }
        })
        media = endpoint
        val listener = object : VoiceSessionListener {
            override fun onVoiceState(state: VoiceCallState) {
                scope.launch {
                    if (!generation.accepts(id)) return@launch
                    val since = _state.value.activeSince ?: if (state.phase == VoiceCallPhase.ACTIVE) android.os.SystemClock.elapsedRealtime() else null
                    _state.value = _state.value.copy(call = state, activeSince = since)
                    if (state.voices.isNotEmpty()) _voices.value = state.voices
                }
            }
            override fun onVoiceClosed(reason: VoiceEndReason?) {
                scope.launch { if (generation.accepts(id)) finish(reason) }
            }
        }
        try {
            call = client.startVoice(host.id, _selectedVoice.value, endpoint, listener).also(endpoint::attach)
            if (!assistantCall) model.pendingRoute.value = "jarvis"
        } catch (_: Exception) { finish(VoiceEndReason.HOST_UNAVAILABLE) }
    }

    private suspend fun askPermission(id: Long): Boolean {
        if (!generation.accepts(id) || !canStart) return false
        if (ContextCompat.checkSelfPermission(app, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) return true
        val reply = CompletableDeferred<Boolean>()
        permission = id to reply
        _permissionRequest.value = id
        return try { reply.await() && generation.accepts(id) && canStart }
        finally {
            if (permission?.first == id) permission = null
            if (_permissionRequest.value == id) _permissionRequest.value = null
        }
    }

    fun permissionLaunched(id: Long) { if (_permissionRequest.value == id) _permissionRequest.value = null }
    fun completePermission(id: Long, granted: Boolean) {
        permission?.takeIf { it.first == id }?.second?.complete(granted)
    }

    fun toggleMute() {
        if (!generation.live) return
        val muted = !_state.value.muted
        _state.value = _state.value.copy(muted = muted)
        media?.muteLocally(muted)
        call?.setMuted(muted)
        model.feedback.both(Haptic.Select, Cue.Select)
    }

    fun toggleSpeaker() {
        if (!generation.live) return
        _state.value = _state.value.copy(speaker = !_state.value.speaker)
        media?.toggleSpeaker()
    }

    fun stop() {
        if (generation.live) model.feedback.both(Haptic.Confirm, Cue.Close)
        finish(null)
    }

    private fun finish(reason: VoiceEndReason?) {
        if (!generation.end()) return
        permission?.second?.complete(false)
        permission = null
        _permissionRequest.value = null
        // Capture stops before the remote lease is released. No automatic reconnect.
        media?.close(); media = null
        call?.stop(); call?.destroy(); call = null
        JarvisService.end(app)
        assistantCall = false
        val host = _state.value.host
        _state.value = JarvisState(host = host, error = reason?.let { message(it, host) }, endReason = reason)
    }

    companion object {
        fun message(reason: VoiceEndReason, host: String): String = when (reason) {
            VoiceEndReason.MICROPHONE_DENIED -> "Allow microphone access in Android Settings to talk with Jarvis."
            VoiceEndReason.AUDIO_UNAVAILABLE -> "The audio connection closed. Start a new call to reconnect."
            VoiceEndReason.SIGN_IN_REQUIRED -> "Sign in to Codex with ChatGPT on $host."
            VoiceEndReason.USAGE_UNAVAILABLE -> "Codex voice usage is unavailable on $host."
            VoiceEndReason.BUSY -> "$host is already on a voice call."
            VoiceEndReason.HOST_UNAVAILABLE -> "$host isn't reachable right now."
            VoiceEndReason.HOST_INCOMPATIBLE -> "Install or update Codex on $host. That device also needs Zeron with remote voice enabled."
            VoiceEndReason.CONNECTION_LOST -> "The call dropped. Start a new one to reconnect."
        }
    }
}
