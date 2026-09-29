package sh.zeron.android.core

import android.app.Application
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.util.Log
import kotlinx.coroutines.CompletableDeferred
import sh.zeron.android.design.Appearance
import sh.zeron.android.design.ThemeMode
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import org.json.JSONObject
import sh.zeron.runtime.RuntimeState
import uniffi.zeron_core.AuthCallback
import uniffi.zeron_core.AuthOrg
import uniffi.zeron_core.ChatConfig
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
import uniffi.zeron_core.NewSession
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
import uniffi.zeron_core.parseAuthCallback
import uniffi.zeron_core.workosAuthorizeUrl
import java.io.File
import java.util.UUID

/** How the app was launched (adb extras mirror the iOS launch arguments). */
data class LaunchOptions(
    val demo: Boolean = false,
    val fast: Boolean = false,
    val longReply: Boolean = false,
    val big: Boolean = false,
    val huge: Boolean = false,
    val noProjects: Boolean = false,
    val signedOut: Boolean = false,
    /** Start in phone mode (the on-device engine). */
    val phone: Boolean = false,
    val route: String? = null,
    val wallpaper: String? = null,
    val wallpaperEffect: String? = null,
)

/** Where agents run: chosen on the sign-in screen, switchable in Settings. */
enum class AppMode {
    /** Remote control of an account's computers via the production edge. */
    Account,

    /** The on-device engine (the `:runtime` guest) over its local edge. */
    Phone,

    /** Offline dataset with a simulated host. */
    Demo,
}

/**
 * App-wide state owner: holds the Rust [CoreClient] for the current mode,
 * republishes its snapshots as flows, and fans session events out to
 * screens. Core events arrive on Rust threads and hop to the main thread here.
 */
class AppModel(private val app: Application) {
    private val scope = MainScope()
    private val main = Handler(Looper.getMainLooper())
    val credentials = CredentialStore(app)
    val wallpaper = WallpaperStore(app)
    val phone by lazy { PhoneEngine(app) }
    val notifier by lazy { Notifier(app) }

    private val _mode = MutableStateFlow<AppMode?>(null)
    val mode: StateFlow<AppMode?> = _mode.asStateFlow()

    /** A route to open once the main UI is up (`chat:<id>` from a notification). */
    val pendingRoute = MutableStateFlow<String?>(null)

    private val _client = MutableStateFlow<CoreClient?>(null)
    val client: StateFlow<CoreClient?> = _client.asStateFlow()

    private val _workspace = MutableStateFlow<WorkspaceSnapshot?>(null)
    val workspace: StateFlow<WorkspaceSnapshot?> = _workspace.asStateFlow()

    private val _connectivity = MutableStateFlow<Connectivity?>(null)
    val connectivity: StateFlow<Connectivity?> = _connectivity.asStateFlow()

    /** Chat ids whose session or composer changed. */
    private val _sessionEvents = MutableSharedFlow<String>(extraBufferCapacity = 256)
    val sessionEvents: SharedFlow<String> = _sessionEvents

    /** Organizations to pick from mid sign-in (more than one). */
    val orgChoice = MutableStateFlow<Pair<List<AuthOrg>, CompletableDeferred<AuthOrg?>>?>(null)
    val signInError = MutableStateFlow<String?>(null)

    private val settings = app.getSharedPreferences("settings", 0)
    private val _appearance = MutableStateFlow(
        Appearance(
            runCatching { ThemeMode.valueOf(settings.getString("theme", "System")!!) }.getOrDefault(ThemeMode.System),
            settings.getBoolean("dynamicColor", false),
        ),
    )
    val appearance: StateFlow<Appearance> = _appearance.asStateFlow()

    fun setAppearance(value: Appearance) {
        _appearance.value = value
        settings.edit().putString("theme", value.mode.name).putBoolean("dynamicColor", value.dynamicColor).apply()
    }

    // The phone's engine has its own devices and projects: its own draft.
    private val draftPrefix get() = if (_mode.value == AppMode.Phone) "draft.phone." else "draft."

    private fun storedDraft() = NewSessionDraft(
        projectId = settings.getString("${draftPrefix}project", null),
        hostId = settings.getString("${draftPrefix}host", null),
        harness = settings.getString("${draftPrefix}harness", null) ?: "claude-code",
        model = settings.getString("${draftPrefix}model", null),
        effort = settings.getString("${draftPrefix}effort", null),
    )

    var lastDraft: NewSessionDraft = storedDraft()
        set(value) {
            field = value
            if (!launch.demo && _mode.value != AppMode.Demo) {
                settings.edit()
                    .putString("${draftPrefix}project", value.projectId)
                    .putString("${draftPrefix}host", value.hostId)
                    .putString("${draftPrefix}harness", value.harness)
                    .putString("${draftPrefix}model", value.model)
                    .putString("${draftPrefix}effort", value.effort)
                    .apply()
            }
        }

    var launch = LaunchOptions()
        private set

    val isDemo: Boolean get() = _client.value?.isDemo() == true
    private var refreshScheduled = false
    private var foreground = false
    private var authState: String? = null

    private val listener = object : ClientListener {
        override fun onEvent(event: ClientEvent) {
            main.post { handle(event) }
        }
    }

    fun boot(options: LaunchOptions) {
        launch = options
        if (_client.value != null) return
        pendingRoute.value = options.route
        if (options.signedOut) {
            credentials.clear()
            settings.edit().remove("mode").apply()
        }
        watchNetwork()
        // `wallpaper <path>` / `wallpaper none` and `wallpaper-effect <name>`:
        // set the wallpaper at launch (screenshots, tests).
        options.wallpaper?.let { path ->
            if (path == "none") wallpaper.remove() else scope.launch { wallpaper.set(File(path)) }
        }
        options.wallpaperEffect?.let { name ->
            WallpaperStore.effects.firstOrNull { it.name.equals(name, ignoreCase = true) }?.let(wallpaper::setEffect)
        }
        val stored = credentials.stored()
        when {
            options.demo -> start(Credentials.Demo(demoOptions()))
            options.phone || settings.getString("mode", null) == AppMode.Phone.name -> startPhone()
            stored != null -> start(stored)
        }
        // Relative times ("4m") and staleness age without events.
        scope.launch {
            while (true) {
                delay(30_000)
                refreshWorkspace()
            }
        }
    }

    fun demoOptions() = DemoOptions(
        fixture = if (launch.noProjects) DemoFixture.NO_PROJECTS else DemoFixture.STANDARD,
        transcriptScale = when {
            launch.huge -> TranscriptScale.Huge
            launch.big -> TranscriptScale.Big
            else -> TranscriptScale.Normal
        },
        streamSpeed = if (launch.fast) StreamSpeed.FAST else StreamSpeed.REALISTIC,
        longReply = launch.longReply,
    )

    private val coreDir get() = File(app.filesDir, "core")

    /** Phone mode's docs: one identity (local/local) for the life of the guest. */
    private val phoneDir get() = File(app.filesDir, "phone")

    /** Local docs belong to one identity: another account starts empty. */
    private fun claimCoreDir(credentials: Credentials) {
        val owner = when (credentials) {
            is Credentials.WorkOs -> "${credentials.userId}/${credentials.orgId}"
            is Credentials.Dev -> "${credentials.userId}/${credentials.orgId}"
            is Credentials.Demo, is Credentials.Local -> return
        }
        val marker = File(coreDir, ".owner")
        if (runCatching { marker.readText() }.getOrNull() != owner) coreDir.deleteRecursively()
        coreDir.mkdirs()
        marker.writeText(owner)
    }

    private fun start(credentials: Credentials, edge: String = edgeUrl, name: String = deviceName()): Boolean {
        val dir = when (credentials) {
            is Credentials.Demo -> File(app.filesDir, "demo")
            is Credentials.Local -> phoneDir
            else -> coreDir.also { claimCoreDir(credentials) }
        }
        dir.mkdirs()
        val config = CoreConfig(
            edgeUrl = edge,
            dataDir = dir.path,
            deviceId = deviceId,
            deviceName = name,
            platform = "android",
            appVersion = app.packageManager.getPackageInfo(app.packageName, 0).versionName ?: "0",
        )
        return try {
            val client = CoreClient(config, credentials, listener)
            if (credentials is Credentials.WorkOs) this.credentials.store(credentials)
            setMode(
                when (credentials) {
                    is Credentials.Demo -> AppMode.Demo
                    is Credentials.Local -> AppMode.Phone
                    else -> AppMode.Account
                },
            )
            _client.value = client
            refreshWorkspace()
            client.preloadSessions()
            true
        } catch (e: Exception) {
            Log.e("Zeron", "core start failed", e)
            false
        }
    }

    fun startDemo() {
        start(Credentials.Demo(demoOptions()))
    }

    // ── modes ──────────────────────────────────────────────────────────────

    /** Demo is never remembered: a relaunch returns to the last real mode. */
    private fun setMode(mode: AppMode?) {
        if (_mode.value == mode) return
        _mode.value = mode
        if (mode != AppMode.Demo) settings.edit().putString("mode", mode?.name).apply()
        lastDraft = storedDraft()
    }

    /** Sign-in screen / Settings: switch where agents run. */
    fun chooseMode(mode: AppMode) {
        if (mode == _mode.value && _client.value != null) return
        closeClient()
        when (mode) {
            AppMode.Demo -> startDemo()
            AppMode.Phone -> startPhone()
            // Signed out of the account: the sign-in screen takes over.
            AppMode.Account -> credentials.stored()?.let { start(it) } ?: setMode(null)
        }
    }

    private var phoneWatch: Job? = null
    private var phoneToken: String? = null

    /**
     * Phone mode: a client exists while the on-device engine runs. A restart
     * (Starting) keeps it — it redials on its own; a stop, reset or failure
     * drops it so the setup screen (state, Start, the log) takes over. Launch
     * restarts the engine unless the user stopped it.
     */
    private fun startPhone() {
        setMode(AppMode.Phone)
        if (settings.getBoolean("phoneAutostart", false) && phone.state.value == RuntimeState.Stopped) phone.start()
        phoneWatch?.cancel()
        phoneWatch = scope.launch {
            phone.state.collect { state ->
                when (state) {
                    is RuntimeState.Running -> if (_client.value == null || phoneToken != state.edgeToken) {
                        dropClient()
                        phoneToken = state.edgeToken
                        start(Credentials.Local(state.edgeToken), state.edgeUrl, state.deviceName)
                    }
                    RuntimeState.Stopped, RuntimeState.NotInstalled, is RuntimeState.Failed -> {
                        phoneToken = null
                        dropClient()
                    }
                    else -> Unit
                }
            }
        }
    }

    /** Start the engine (the user asked): relaunches bring it back up. */
    fun startEngine() {
        settings.edit().putBoolean("phoneAutostart", true).apply()
        phone.start()
    }

    fun stopEngine() {
        settings.edit().putBoolean("phoneAutostart", false).apply()
        phone.stop()
    }

    /**
     * Wipe the guest and this app's copy of its sessions. Runs in the app's
     * scope: the engine stopping swaps the screen that asked for it away.
     */
    fun resetEngine() {
        settings.edit().putBoolean("phoneAutostart", false).apply()
        if (_mode.value == AppMode.Phone) dropClient()
        phoneToken = null
        scope.launch {
            phone.reset()
            phoneDir.deleteRecursively()
        }
    }

    val edgeUrl: String get() = authProductionEdgeUrl()

    private val deviceId: String
        get() {
            val prefs = app.getSharedPreferences("device", 0)
            prefs.getString("deviceId", null)?.let { return it }
            val id = "android-" + UUID.randomUUID().toString().take(8)
            prefs.edit().putString("deviceId", id).apply()
            return id
        }

    private fun deviceName(): String =
        Settings.Global.getString(app.contentResolver, Settings.Global.DEVICE_NAME) ?: Build.MODEL

    // ── sign-in ────────────────────────────────────────────────────────────

    /** The WorkOS authorize URL to open in a Custom Tab. */
    fun beginSignIn(): String {
        val state = UUID.randomUUID().toString()
        authState = state
        return workosAuthorizeUrl(state)
    }

    /** `zeron://callback?code=…`: code → tokens → org → client. */
    fun handleCallback(url: String) {
        when (val cb = parseAuthCallback(url)) {
            is AuthCallback.Code -> scope.launch { signIn(cb.code) }
            is AuthCallback.Error -> signInError.value = cb.description ?: cb.error
            null -> Unit
        }
    }

    private suspend fun signIn(code: String) {
        try {
            val exchange = authExchangeCode(edgeUrl, code)
            val orgs = authListOrgs(edgeUrl, exchange.tokens.accessToken)
            if (orgs.isEmpty()) {
                signInError.value = "This account isn't in an organization yet."
                return
            }
            val org = if (orgs.size == 1) orgs[0] else {
                val choice = CompletableDeferred<AuthOrg?>()
                orgChoice.value = orgs to choice
                choice.await().also { orgChoice.value = null } ?: return
            }
            val tokens = authRefresh(edgeUrl, exchange.tokens.refreshToken, org.organizationId)
            val user = exchange.user
            val name = listOfNotNull(user.firstName, user.lastName).map { it.trim() }.filter { it.isNotEmpty() }.joinToString(" ")
            credentials.profile = CredentialStore.Profile(name.ifEmpty { null }, user.email, org.name)
            if (!start(Credentials.WorkOs(user.id, org.organizationId, tokens))) {
                signInError.value = "Couldn't open your workspace. Try signing in again."
            }
        } catch (e: Exception) {
            signInError.value = e.message ?: "Sign-in failed"
        }
    }

    /** Leave demo / sign out of the account / leave phone mode (the engine keeps running). */
    fun signOut() {
        val was = _mode.value
        closeClient()
        setMode(null)
        if (was == AppMode.Account) {
            credentials.clear()
            coreDir.deleteRecursively()
        }
    }

    private fun dropClient() {
        _client.value?.shutdown()
        _client.value = null
        _workspace.value = null
        _connectivity.value = null
    }

    private fun closeClient() {
        phoneWatch?.cancel()
        phoneWatch = null
        phoneToken = null
        dropClient()
    }

    val accountName: String
        get() = when {
            isDemo -> "Demo"
            _mode.value == AppMode.Phone -> "This phone"
            _client.value == null -> "Signed out"
            else -> credentials.profile.let { it.name ?: it.email ?: "Signed in" }
        }

    val accountDetail: String
        get() = when {
            isDemo -> "Offline demo workspace"
            _mode.value == AppMode.Phone -> "Agents run on this device"
            else -> credentials.profile.let { p ->
                listOfNotNull(if (p.name != null) p.email else null, p.orgName).joinToString(" · ").ifEmpty { "Zeron account" }
            }
        }

    // ── events ─────────────────────────────────────────────────────────────

    private fun handle(event: ClientEvent) {
        when (event) {
            is ClientEvent.WorkspaceChanged -> scheduleRefresh()
            is ClientEvent.SessionChanged -> _sessionEvents.tryEmit(event.chatId)
            is ClientEvent.ComposerChanged -> _sessionEvents.tryEmit(event.chatId)
            is ClientEvent.ConnectivityChanged -> _connectivity.value = event.connectivity
            is ClientEvent.AuthRefreshed -> credentials.updateTokens(event.tokens)
            // The phone's edge never expires a token; a rejected one waits for the next engine start.
            is ClientEvent.AuthExpired -> if (_mode.value == AppMode.Phone) {
                phoneToken = null
                dropClient()
            } else {
                signOut()
            }
        }
    }

    /** Coalesce bursts of registry frames into one rebuild per loop turn. */
    private fun scheduleRefresh() {
        if (refreshScheduled) return
        refreshScheduled = true
        main.post {
            refreshScheduled = false
            refreshWorkspace()
        }
    }

    fun refreshWorkspace() {
        val client = _client.value ?: return
        val previous = _workspace.value
        val next = client.workspace()
        _workspace.value = next
        if (_mode.value == AppMode.Phone) notifier.onWorkspace(previous, next, foreground)
    }

    fun onForeground() {
        foreground = true
        _client.value?.onForeground()
        refreshWorkspace()
    }

    fun onBackground() {
        foreground = false
        _client.value?.onBackground()
    }

    /** Pull to refresh: redial and re-probe, then settle for a beat. */
    suspend fun refresh() {
        onForeground()
        delay(700)
    }

    private fun watchNetwork() {
        val cm = app.getSystemService(ConnectivityManager::class.java) ?: return
        cm.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                main.post { _client.value?.setNetworkOnline(true) }
            }

            override fun onLost(network: Network) {
                main.post {
                    val active = cm.getNetworkCapabilities(cm.activeNetwork)
                    val online = active?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
                    _client.value?.setNetworkOnline(online)
                }
            }
        })
    }

    // ── writes ─────────────────────────────────────────────────────────────

    private inline fun attempt(what: String, body: () -> Unit) {
        try {
            body()
        } catch (e: Exception) {
            Log.w("Zeron", "$what failed", e)
        }
    }

    fun setPinned(id: String, pinned: Boolean) = attempt("pin") {
        val c = _client.value ?: return
        if (pinned) c.pinSession(id) else c.unpinSession(id)
    }

    fun archive(id: String) = attempt("archive") { _client.value?.archiveSession(id) }
    fun unarchive(id: String) = attempt("unarchive") { _client.value?.unarchiveSession(id) }
    fun rename(id: String, title: String) = attempt("rename") { _client.value?.renameSession(id, title) }
    fun setSectionCollapsed(id: String, collapsed: Boolean) = attempt("collapse") { _client.value?.setSectionCollapsed(id, collapsed) }

    fun row(id: String): SessionRow? = _client.value?.sessionRow(id)

    /** Devices that run agents (this phone's engine in phone mode). */
    fun executionDevices(): List<DeviceView> = runCatching { _client.value?.executionDevices() }.getOrNull().orEmpty()

    // ── host calls ─────────────────────────────────────────────────────────

    /** Untyped engine RPC (harness installs, agent sign-ins). Throws on failure. */
    suspend fun hostCall(deviceId: String, method: String, params: JSONObject = JSONObject()): Any {
        val c = _client.value ?: throw IllegalStateException("Not connected")
        return Agents.parse(c.hostCall(deviceId, method, params.toString()))
    }

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
        return createProject(deviceId, path)
    }

    /** Phone mode: an empty repository under /home/zeron/projects, as a project. */
    suspend fun newPhoneProject(deviceId: String, name: String): Result<String> =
        createProject(deviceId, phone.newProject(name).getOrElse { return Result.failure(it) })

    private suspend fun createProject(deviceId: String, path: String): Result<String> {
        val c = _client.value ?: return Result.failure(IllegalStateException("Not connected"))
        return runCatching { c.createProject(deviceId, path, true) }.onSuccess { refreshWorkspace() }
    }

    /** Create the chat and send its first message. */
    fun createSession(draft: NewSessionDraft, text: String, attachments: List<uniffi.zeron_core.OutgoingAttachment> = emptyList()): String? {
        val client = _client.value ?: return null
        val target = when {
            draft.projectId != null -> SessionTarget.Project(draft.projectId)
            draft.hostId != null -> SessionTarget.Projectless(draft.hostId)
            else -> return null
        }
        val config = ChatConfig(draft.harness, draft.model, draft.effort, emptyMap(), SandboxLevel.WORKSPACE_WRITE)
        return try {
            val chatId = client.createSession(NewSession(target, config, if (draft.worktree) null else draft.branch, null, null))
            val handle = client.openSession(chatId)
            val project = _workspace.value?.projects?.firstOrNull { it.id == draft.projectId }
            val worktree = if (draft.worktree && project != null) WorktreeSpec(project.path, draft.branch ?: "HEAD", project.id) else null
            handle.send(SendRequest(text, attachments, worktree, BusyPolicy.QUEUE))
            refreshWorkspace()
            chatId
        } catch (e: Exception) {
            Log.w("Zeron", "create session failed", e)
            null
        }
    }
}

/** Human wording for core errors. */
fun Throwable.userMessage(): String = when (this) {
    is CoreException.HostUnavailable ->
        if (Agents.isTimeout(reason)) "The device took too long to answer." else "The device isn't reachable right now."
    is CoreException.Unsupported -> "Not supported by this device's engine."
    is CoreException.Closed -> "Not connected."
    // Host errors arrive as "Method: reason" — the reason is what people read.
    is CoreException.HostException -> reason.substringAfter(": ", reason).ifBlank { "The device couldn't do that." }
    is CoreException.NotFound -> reason
    is CoreException.InvalidArgument -> reason
    is CoreException.Network -> reason
    is CoreException.Auth -> reason
    is CoreException.Storage -> reason
    is CoreException.NotImplemented -> reason
    is CoreException.Internal -> reason
    else -> message ?: "Something went wrong."
}

/** The new-session page's options (kept across launches). */
data class NewSessionDraft(
    val projectId: String? = null,
    val hostId: String? = null,
    val harness: String = "claude-code",
    val model: String? = null,
    val effort: String? = null,
    val branch: String? = null,
    val worktree: Boolean = false,
)
