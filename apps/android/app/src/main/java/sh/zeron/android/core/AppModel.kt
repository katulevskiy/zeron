package sh.zeron.android.core

import android.app.Application
import android.os.Build
import android.provider.Settings
import android.util.Log
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import sh.zeron.android.BuildConfig
import sh.zeron.android.design.Appearance
import sh.zeron.runtime.RuntimeState
import uniffi.zeron_core.AuthCallback
import uniffi.zeron_core.AuthOrg
import uniffi.zeron_core.ChatConfig
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.ClientEvent
import uniffi.zeron_core.ClientListener
import uniffi.zeron_core.Connectivity
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.CoreConfig
import uniffi.zeron_core.CoreException
import uniffi.zeron_core.Credentials
import uniffi.zeron_core.DemoFixture
import uniffi.zeron_core.DemoOptions
import uniffi.zeron_core.DeviceView
import uniffi.zeron_core.FileMatch
import uniffi.zeron_core.FolderListing
import uniffi.zeron_core.NewSession
import uniffi.zeron_core.OutgoingAttachment
import uniffi.zeron_core.ProjectView
import uniffi.zeron_core.SandboxLevel
import uniffi.zeron_core.SendRequest
import uniffi.zeron_core.BusyPolicy
import uniffi.zeron_core.SessionRow
import uniffi.zeron_core.SessionTarget
import uniffi.zeron_core.StreamSpeed
import uniffi.zeron_core.TranscriptScale
import uniffi.zeron_core.WorkspaceSnapshot
import uniffi.zeron_core.WorktreeSpec
import uniffi.zeron_core.authExchangeCode
import uniffi.zeron_core.authListOrgs
import uniffi.zeron_core.authProductionEdgeUrl
import uniffi.zeron_core.authRefresh
import uniffi.zeron_core.fallbackHarnesses
import uniffi.zeron_core.fallbackModels
import uniffi.zeron_core.parseAuthCallback
import uniffi.zeron_core.workosAuthorizeUrl
import java.io.File
import java.util.UUID

/** An image staged in a composer before sending (JPEG bytes + thumbnail). */
class StagedImage(val id: String, val name: String, val bytes: ByteArray, val thumbnail: android.graphics.Bitmap) {
    val outgoing get() = OutgoingAttachment(name, "image/jpeg", bytes)
}

/** What a new session will be created with (persisted across launches). */
data class NewSessionDraft(
    val projectId: String? = null,
    val hostId: String? = null,
    val branch: String? = null,
    val worktree: Boolean = false,
    val harness: String = "claude-code",
    val model: String? = null,
    val effort: String? = null,
) {
    fun toJson(): String = JSONObject()
        .put("projectId", projectId).put("hostId", hostId).put("branch", branch).put("worktree", worktree)
        .put("harness", harness).put("model", model).put("effort", effort).toString()

    companion object {
        fun fromJson(raw: String?): NewSessionDraft = runCatching {
            val o = JSONObject(raw!!)
            fun s(k: String) = if (o.isNull(k)) null else o.optString(k).ifEmpty { null }
            NewSessionDraft(s("projectId"), s("hostId"), s("branch"), o.optBoolean("worktree"), s("harness") ?: "claude-code", s("model"), s("effort"))
        }.getOrDefault(NewSessionDraft())
    }
}

/** A harness + model the new-session picker offers. */
data class ModelChoice(val harness: String, val harnessLabel: String, val id: String, val label: String, val efforts: List<String>, val description: String?)

/** Sign-in progress (Account mode). */
sealed interface AuthStage {
    data object Idle : AuthStage
    data object Browser : AuthStage
    data object Exchanging : AuthStage
    data class PickOrg(val orgs: List<AuthOrg>) : AuthStage
    data class Failed(val message: String) : AuthStage
}

/**
 * App-wide state owner: holds the Rust [CoreClient] for the current mode,
 * republishes its snapshots as flows, and fans change events out to screens.
 * Core events arrive on Rust threads and hop to the main thread here.
 */
class AppModel(val app: Application) {
    val prefs = Prefs(app)
    val secure = SecureStore(app)
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    val phone by lazy { PhoneEngine(app) }
    val notifier by lazy { Notifier(app) }

    private val _mode = MutableStateFlow(prefs.mode)
    val mode: StateFlow<AppMode?> = _mode.asStateFlow()

    private val _client = MutableStateFlow<CoreClient?>(null)
    val client: StateFlow<CoreClient?> = _client.asStateFlow()

    private val _workspace = MutableStateFlow<WorkspaceSnapshot?>(null)
    val workspace: StateFlow<WorkspaceSnapshot?> = _workspace.asStateFlow()

    private val _connectivity = MutableStateFlow<Connectivity?>(null)
    val connectivity: StateFlow<Connectivity?> = _connectivity.asStateFlow()

    /** Chat ids whose transcript/composer advanced (sessions filter their own). */
    val sessionPulse = MutableSharedFlow<String>(extraBufferCapacity = 256)

    private val _auth = MutableStateFlow<AuthStage>(AuthStage.Idle)
    val auth: StateFlow<AuthStage> = _auth.asStateFlow()

    val appearance = MutableStateFlow(prefs.appearance)

    /** Sessions that should open when the UI is ready (notification taps). */
    val openRequests = MutableSharedFlow<String>(extraBufferCapacity = 4)

    var foreground = false
        private set

    private val refreshes = Channel<Unit>(Channel.CONFLATED)
    private var clock: Job? = null
    private var phoneWatch: Job? = null

    init {
        ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) {
                foreground = true
                _client.value?.onForeground()
                requestRefresh()
            }

            override fun onStop(owner: LifecycleOwner) {
                foreground = false
                _client.value?.onBackground()
            }
        })
        scope.launch {
            for (ignored in refreshes) {
                val c = _client.value ?: continue
                // The snapshot crosses FFI whole: build it off the main thread.
                val ws = withContext(Dispatchers.Default) { runCatching { c.workspace() }.getOrNull() } ?: continue
                if (_client.value !== c) continue
                val previous = _workspace.value
                _workspace.value = ws
                if (_mode.value == AppMode.Phone) notifier.onWorkspace(previous, ws, foreground)
            }
        }
        resume()
    }

    // ── lifecycle ─────────────────────────────────────────────────────────

    /** Launch with what was chosen last time (nothing = onboarding). */
    private fun resume() {
        when (_mode.value) {
            AppMode.Demo -> startDemo()
            AppMode.Account -> secure.account?.let { startAccount(it) }
            AppMode.Phone -> watchPhone()
            null -> Unit
        }
    }

    /** First run / Settings: switch engines. Stops the current client. */
    fun chooseMode(mode: AppMode) {
        stopClient()
        prefs.mode = mode
        _mode.value = mode
        when (mode) {
            AppMode.Demo -> startDemo()
            AppMode.Account -> secure.account?.let { startAccount(it) }
            AppMode.Phone -> watchPhone()
        }
    }

    /** Back to the first-run chooser. */
    fun resetMode() {
        stopClient()
        prefs.mode = null
        _mode.value = null
    }

    private fun stopClient() {
        phoneWatch?.cancel()
        phoneWatch = null
        phoneToken = null
        closeClient()
    }

    private fun closeClient() {
        clock?.cancel()
        _client.value?.shutdown()
        _client.value = null
        _workspace.value = null
        _connectivity.value = null
    }

    fun demoOptions(): DemoOptions = DemoOptions(
        fixture = DemoFixture.STANDARD,
        transcriptScale = when (prefs.demoScale) {
            "big" -> TranscriptScale.Big
            "huge" -> TranscriptScale.Huge
            else -> TranscriptScale.Normal
        },
        streamSpeed = if (prefs.demoFast) StreamSpeed.FAST else StreamSpeed.REALISTIC,
        longReply = false,
    )

    private fun startDemo() {
        // Demo data never persists between launches.
        val dir = File(app.filesDir, "core/demo").apply { deleteRecursively(); mkdirs() }
        startClient(Credentials.Demo(demoOptions()), dir, authProductionEdgeUrl(), app.deviceName())
    }

    /** Restart Demo (developer options changed the dataset). */
    fun restartDemo() {
        if (_mode.value != AppMode.Demo) return
        stopClient()
        startDemo()
    }

    private fun startAccount(a: SecureStore.Account) {
        // Local docs belong to one identity: another account starts empty.
        val dir = File(app.filesDir, "core/account")
        val marker = File(dir, ".owner")
        val owner = "${a.userId}/${a.orgId}"
        if (runCatching { marker.readText() }.getOrNull() != owner) dir.deleteRecursively()
        dir.mkdirs()
        marker.writeText(owner)
        startClient(Credentials.WorkOs(a.userId, a.orgId, a.tokens), dir, authProductionEdgeUrl(), app.deviceName())
    }

    /**
     * Phone mode: a client exists while the on-device engine runs. A restart
     * (Starting) keeps the client — it reconnects on its own; a stop, reset or
     * failure drops it so the setup screen takes over.
     */
    private fun watchPhone() {
        phoneWatch?.cancel()
        phoneWatch = scope.launch {
            phone.state.collect { state ->
                when (state) {
                    is RuntimeState.Running -> if (_client.value == null || phoneToken != state.edgeToken) {
                        phoneToken = state.edgeToken
                        // One identity (local/local) forever: the docs persist across restarts.
                        val dir = File(app.filesDir, "core/phone").apply { mkdirs() }
                        startClient(Credentials.Local(state.edgeToken), dir, state.edgeUrl, state.deviceName)
                    }
                    RuntimeState.Stopped, RuntimeState.NotInstalled, is RuntimeState.Failed -> if (_client.value != null) {
                        phoneToken = null
                        closeClient()
                    }
                    else -> Unit
                }
            }
        }
    }

    private var phoneToken: String? = null

    private fun startClient(credentials: Credentials, dir: File, edgeUrl: String, deviceName: String) {
        val config = CoreConfig(
            edgeUrl = edgeUrl,
            dataDir = dir.absolutePath,
            deviceId = prefs.deviceId,
            deviceName = deviceName,
            platform = "android",
            appVersion = BuildConfig.VERSION_NAME,
        )
        try {
            _client.value?.shutdown()
            val c = CoreClient(config, credentials, Bridge(this))
            _client.value = c
            _connectivity.value = c.connectivity()
            c.preloadSessions()
            requestRefresh()
            clock?.cancel()
            // Relative times ("4m") and the 45 s staleness age without events.
            clock = scope.launch {
                while (true) {
                    delay(30_000)
                    requestRefresh()
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "core start failed", e)
            _client.value = null
        }
    }

    fun requestRefresh() {
        refreshes.trySend(Unit)
    }

    /** Pull to refresh: redial rooms and re-probe (the foreground resync). */
    fun refreshNow() {
        _client.value?.onForeground()
        requestRefresh()
    }

    internal fun handle(event: ClientEvent) {
        when (event) {
            is ClientEvent.WorkspaceChanged -> requestRefresh()
            is ClientEvent.SessionChanged -> sessionPulse.tryEmit(event.chatId)
            is ClientEvent.ComposerChanged -> sessionPulse.tryEmit(event.chatId)
            is ClientEvent.ConnectivityChanged -> _connectivity.value = event.connectivity
            is ClientEvent.AuthRefreshed -> secure.updateTokens(event.tokens)
            is ClientEvent.AuthExpired -> {
                Log.w(TAG, "auth expired: ${event.reason}")
                stopClient()
                _auth.value = AuthStage.Failed("Your session expired. Sign in again.")
            }
        }
    }

    // ── account sign-in (WorkOS) ──────────────────────────────────────────

    /** The authorize URL to open in a Custom Tab (state kept for the callback). */
    fun beginSignIn(): String {
        val state = UUID.randomUUID().toString()
        prefs.pendingAuthState = state
        _auth.value = AuthStage.Browser
        return workosAuthorizeUrl(state)
    }

    private var pendingExchange: uniffi.zeron_core.AuthExchange? = null

    /** `zeron://callback?code=…&state=…` arrived. */
    fun handleAuthCallback(url: String) {
        when (val cb = parseAuthCallback(url)) {
            is AuthCallback.Error -> _auth.value = AuthStage.Failed(cb.description ?: cb.error)
            is AuthCallback.Code -> {
                // Only a callback for the sign-in this app started (any app can fire zeron://).
                val pending = prefs.pendingAuthState
                if (pending == null || cb.state != pending) {
                    _auth.value = AuthStage.Failed("Sign-in didn't complete. Try again.")
                    return
                }
                prefs.pendingAuthState = null
                _auth.value = AuthStage.Exchanging
                scope.launch {
                    try {
                        val edge = authProductionEdgeUrl()
                        val exchange = authExchangeCode(edge, cb.code)
                        val orgs = authListOrgs(edge, exchange.tokens.accessToken)
                        when {
                            orgs.isEmpty() -> _auth.value = AuthStage.Failed("This account isn't in an organization yet.")
                            orgs.size == 1 -> finishSignIn(exchange, orgs[0])
                            else -> {
                                pendingExchange = exchange
                                _auth.value = AuthStage.PickOrg(orgs)
                            }
                        }
                    } catch (e: Exception) {
                        _auth.value = AuthStage.Failed(e.userMessage())
                    }
                }
            }
            null -> _auth.value = AuthStage.Failed("Sign-in didn't complete. Try again.")
        }
    }

    fun pickOrg(org: AuthOrg) {
        val exchange = pendingExchange ?: return
        pendingExchange = null
        _auth.value = AuthStage.Exchanging
        scope.launch {
            try {
                finishSignIn(exchange, org)
            } catch (e: Exception) {
                _auth.value = AuthStage.Failed(e.userMessage())
            }
        }
    }

    fun cancelSignIn() {
        pendingExchange = null
        _auth.value = AuthStage.Idle
    }

    private suspend fun finishSignIn(exchange: uniffi.zeron_core.AuthExchange, org: AuthOrg) {
        val tokens = authRefresh(authProductionEdgeUrl(), exchange.tokens.refreshToken, org.organizationId)
        val u = exchange.user
        val name = listOfNotNull(u.firstName?.trim(), u.lastName?.trim()).filter { it.isNotEmpty() }.joinToString(" ").ifEmpty { null }
        val account = SecureStore.Account(u.id, org.organizationId, tokens)
        secure.account = account
        secure.profile = SecureStore.Profile(name, u.email, org.name)
        prefs.mode = AppMode.Account
        _mode.value = AppMode.Account
        startAccount(account)
        _auth.value = AuthStage.Idle
    }

    /** Sign out of the account: forget its local docs. */
    fun signOut() {
        stopClient()
        secure.clearAccount()
        File(app.filesDir, "core/account").deleteRecursively()
        _auth.value = AuthStage.Idle
    }

    // ── reads ─────────────────────────────────────────────────────────────

    val isDemo get() = _client.value?.isDemo() ?: false

    fun row(chatId: String): SessionRow? {
        val ws = _workspace.value
        ws?.let { w ->
            (w.front.pinned.asSequence() + w.front.sections.asSequence().flatMap { it.sessions } + w.front.recent + w.archived + w.projectless)
                .firstOrNull { it.id == chatId }?.let { return it }
        }
        return _client.value?.sessionRow(chatId)
    }

    fun projects(): List<ProjectView> = _workspace.value?.projects ?: emptyList()

    fun executionDevices(): List<DeviceView> = runCatching { _client.value?.executionDevices() }.getOrNull() ?: emptyList()

    fun deviceName(id: String): String = _workspace.value?.devices?.firstOrNull { it.id == id }?.name ?: "Unknown device"

    val accountName: String
        get() = when (_mode.value) {
            AppMode.Demo -> "Demo"
            AppMode.Phone -> "This phone"
            else -> secure.profile.let { it.name ?: it.email } ?: if (_client.value != null) "Signed in" else "Signed out"
        }

    val accountDetail: String
        get() = when (_mode.value) {
            AppMode.Demo -> "Offline demo workspace"
            AppMode.Phone -> "Agents run on this device"
            else -> secure.profile.let { p -> listOfNotNull(if (p.name != null) p.email else null, p.orgName).joinToString(" · ") }.ifEmpty { "Zeron account" }
        }

    // ── writes ────────────────────────────────────────────────────────────

    private inline fun attempt(what: String, body: CoreClient.() -> Unit) {
        val c = _client.value ?: return
        try {
            c.body()
        } catch (e: Exception) {
            Log.w(TAG, "$what failed", e)
        }
    }

    fun setPinned(id: String, pinned: Boolean) = attempt("pin") { if (pinned) pinSession(id) else unpinSession(id) }
    fun archive(id: String) = attempt("archive") { archiveSession(id) }
    fun unarchive(id: String) = attempt("unarchive") { unarchiveSession(id) }
    fun rename(id: String, title: String) = attempt("rename") { renameSession(id, title) }
    fun delete(id: String) = attempt("delete") { deleteSession(id) }
    fun moveToSection(id: String, section: String?) = attempt("assign") { assignSection(id, section) }
    fun createSection(name: String) = attempt("section") { createSection(name) }
    fun renameSection(id: String, name: String) = attempt("rename section") { renameSection(id, name) }
    fun deleteSection(id: String) = attempt("delete section") { deleteSection(id) }
    fun setSectionCollapsed(id: String, collapsed: Boolean) = attempt("collapse") { setSectionCollapsed(id, collapsed) }
    fun movePin(id: String, after: String?, before: String?) = attempt("move pin") { movePin(id, after, before) }
    fun markSeen(id: String) = attempt("seen") { markSeen(id) }
    fun renameProject(id: String, name: String?) = attempt("rename project") { renameProject(id, name) }
    fun deleteProject(id: String) = attempt("delete project") { deleteProject(id) }

    fun setAppearance(a: Appearance) {
        prefs.appearance = a
        appearance.value = a
    }

    /** Core-ranked matches (titles, projects, branches, previews); empty query = Recent. */
    fun search(query: String): List<SessionRow> {
        val c = _client.value ?: return emptyList()
        val q = query.trim()
        if (q.isEmpty()) return _workspace.value?.front?.recent ?: emptyList()
        return runCatching { c.search(q, 60u) }.getOrDefault(emptyList()).map { it.session }
    }

    /** The new-session page's picks, kept across launches. */
    var lastDraft: NewSessionDraft
        get() = NewSessionDraft.fromJson(prefs.newSessionDraft)
        set(v) {
            prefs.newSessionDraft = v.toJson()
        }

    /** Text typed on the new-session page and not sent yet (this process only). */
    var newSessionText = ""
    var newSessionImages: List<StagedImage> = emptyList()

    /**
     * Clone a repository and make it a project. On this phone the runtime runs
     * `git clone` into /home/zeron/projects (the engine's CloneRepo would put
     * it under its data dir); elsewhere the device's engine clones it.
     */
    suspend fun cloneRepo(deviceId: String, url: String): Result<String> {
        val path = if (_mode.value == AppMode.Phone) {
            phone.clone(url).getOrElse { return Result.failure(it) }
        } else {
            runCatching {
                val reply = hostCall(deviceId, "CloneRepo", JSONObject().put("url", url.trim())) as? JSONObject
                reply?.optString("path")?.ifEmpty { null } ?: error("The device didn't say where it cloned to.")
            }.getOrElse { return Result.failure(it) }
        }
        return createProject(deviceId, path, true)
    }

    // ── host calls ────────────────────────────────────────────────────────

    suspend fun models(deviceId: String): List<ModelChoice> {
        val c = _client.value ?: return emptyList()
        val harnesses = runCatching { c.listHarnesses(deviceId) }.getOrElse { fallbackHarnesses() }
        return harnesses.filter { it.offered }.flatMap { h ->
            val models = runCatching { c.listModels(deviceId, h.id) }.getOrElse { fallbackModels(h.id) }
            models.map { ModelChoice(h.id, h.label, it.id, it.label, it.reasoningLevels, it.description) }
        }
    }

    fun catalogModels(): List<ModelChoice> = fallbackHarnesses().filter { it.offered }.flatMap { h ->
        fallbackModels(h.id).map { ModelChoice(h.id, h.label, it.id, it.label, it.reasoningLevels, it.description) }
    }

    suspend fun listFolders(deviceId: String, path: String?): FolderListing? =
        runCatching { _client.value?.listFolders(deviceId, path) }.onFailure { Log.w(TAG, "list folders", it) }.getOrNull()

    suspend fun createProject(deviceId: String, path: String, git: Boolean): Result<String> {
        val c = _client.value ?: return Result.failure(IllegalStateException("Not connected"))
        return runCatching { c.createProject(deviceId, path, git) }.onSuccess { requestRefresh() }
    }

    suspend fun searchFiles(deviceId: String, chatId: String?, spaceId: String?, query: String): List<FileMatch> =
        runCatching { _client.value?.searchFiles(deviceId, chatId, spaceId, query) }.getOrNull() ?: emptyList()

    suspend fun refs(projectId: String): List<String> {
        val c = _client.value ?: return emptyList()
        val p = projects().firstOrNull { it.id == projectId } ?: return emptyList()
        val refs = runCatching { c.listRefs(p.deviceId, p.path) }.getOrElse { emptyList() }
        return refs.sortedByDescending { it.current }.map { it.name }
    }

    /** Untyped engine RPC (harness installs, agent logins). Throws on failure. */
    suspend fun hostCall(deviceId: String, method: String, params: JSONObject = JSONObject()): Any {
        val c = _client.value ?: throw IllegalStateException("Not connected")
        val raw = c.hostCall(deviceId, method, params.toString())
        return when (raw.trimStart().firstOrNull()) {
            '[' -> JSONArray(raw)
            '{' -> JSONObject(raw)
            else -> raw
        }
    }

    /** Create the chat, open it, and send the first message. */
    fun createSession(draft: NewSessionDraft, text: String, images: List<StagedImage>): Result<String> {
        val c = _client.value ?: return Result.failure(IllegalStateException("Not connected"))
        val target = when {
            draft.projectId != null -> SessionTarget.Project(draft.projectId)
            draft.hostId != null -> SessionTarget.Projectless(draft.hostId)
            else -> return Result.failure(IllegalArgumentException("Choose a project or a device that can run it."))
        }
        return runCatching {
            val config = ChatConfig(draft.harness, draft.model, draft.effort, emptyMap(), SandboxLevel.WORKSPACE_WRITE)
            val chatId = c.createSession(NewSession(target, config, if (draft.worktree) null else draft.branch, null, null))
            val handle = c.openSession(chatId)
            val project = projects().firstOrNull { it.id == draft.projectId }
            val worktree = if (draft.worktree && project != null) WorktreeSpec(project.path, draft.branch ?: "HEAD", project.id) else null
            handle.send(SendRequest(text, images.map { it.outgoing }, worktree, BusyPolicy.QUEUE))
            prefs.newSessionDraft = draft.toJson()
            requestRefresh()
            chatId
        }.onFailure { Log.w(TAG, "create session failed", it) }
    }

    companion object {
        const val TAG = "Zeron"
    }
}

/** Core events arrive on Rust threads; hop to main. */
private class Bridge(private val model: AppModel) : ClientListener {
    private val main = android.os.Handler(android.os.Looper.getMainLooper())
    override fun onEvent(event: ClientEvent) {
        main.post { model.handle(event) }
    }
}

fun Application.deviceName(): String =
    Settings.Global.getString(contentResolver, "device_name")?.takeIf { it.isNotBlank() } ?: Build.MODEL

/**
 * The core's own wording for an error (the `CoreError` variant's message;
 * renamed `detail` in Kotlin — see crates/mobile/uniffi.toml).
 */
val CoreException.detailText: String?
    get() = when (this) {
        is CoreException.NotFound -> detail
        is CoreException.InvalidArgument -> detail
        is CoreException.HostUnavailable -> detail
        is CoreException.Unsupported -> detail
        is CoreException.HostException -> detail
        is CoreException.Network -> detail
        is CoreException.Auth -> detail
        is CoreException.Storage -> detail
        is CoreException.NotImplemented -> detail
        is CoreException.Internal -> detail
        is CoreException.Closed -> null
    }

/** Human wording for core errors. */
fun Throwable.userMessage(): String = when (this) {
    is CoreException.HostUnavailable ->
        if (Agents.isTimeout(detail)) "The device took too long to answer." else "The device isn't reachable right now."
    is CoreException.Unsupported -> "Not supported by this device's engine."
    is CoreException.Closed -> "Not connected."
    // Host errors arrive as "Method: reason" — the reason is what people read.
    is CoreException.HostException -> detail.substringAfter(": ", detail).ifBlank { "The device couldn't do that." }
    is CoreException -> detailText?.ifBlank { null } ?: "Something went wrong."
    else -> message ?: "Something went wrong."
}

/** Is a session's live state one of the "looks at me" states? */
fun ChatIndicator.isLive() = this == ChatIndicator.WORKING || this == ChatIndicator.AWAITING_INPUT
