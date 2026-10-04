package sh.zeron.android.voice.assistant

import android.app.KeyguardManager
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.service.voice.VoiceInteractionService
import android.service.voice.VoiceInteractionSession
import android.view.View
import android.view.WindowManager
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.platform.ViewCompositionStrategy
import androidx.lifecycle.*
import androidx.savedstate.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import sh.zeron.android.MainActivity
import sh.zeron.android.R
import sh.zeron.android.ZeronApplication
import java.lang.ref.WeakReference

/** System-owned TYPE_VOICE_INTERACTION window. Activity/process foreground is
 * intentionally not forged for this overlay. */
class JarvisAssistantSession(context: Context) : VoiceInteractionSession(context),
    LifecycleOwner, SavedStateRegistryOwner, ViewModelStoreOwner {
    private val registry = LifecycleRegistry(this)
    override val lifecycle: Lifecycle get() = registry
    private val savedState = SavedStateRegistryController.create(this)
    override val savedStateRegistry: SavedStateRegistry get() = savedState.savedStateRegistry
    override val viewModelStore = ViewModelStore()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val model get() = (context.applicationContext as ZeronApplication).model
    private var view: ComposeView? = null
    private var cameraGeneration: Long? = null
    private var cameraToken: String? = null
    private var cameraOpen = false
    private val _cameraBusy = MutableStateFlow(false)
    val cameraBusy = _cameraBusy.asStateFlow()
    private var startJob: Job? = null
    private var shown = false
    private var keepCall = false
    private var permissionGeneration: Long? = null
    private val _preparing = MutableStateFlow(false)
    val preparing = _preparing.asStateFlow()
    private val _notice = MutableStateFlow<String?>(null)
    val notice = _notice.asStateFlow()
    private val _notifications = MutableStateFlow(false)
    val notifications = _notifications.asStateFlow()
    private fun trace(event: String) {
        if (context.applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE != 0)
            android.util.Log.d("JarvisAssistant", event)
    }
    private fun refreshNotificationPermission() {
        _notifications.value = android.os.Build.VERSION.SDK_INT < 33 ||
            androidx.core.content.ContextCompat.checkSelfPermission(context, android.Manifest.permission.POST_NOTIFICATIONS) == android.content.pm.PackageManager.PERMISSION_GRANTED
    }
    init { setTheme(R.style.Theme_Zeron_Assistant) }

    override fun onCreate() {
        super.onCreate()
        savedState.performAttach()
        savedState.performRestore(null)
        registry.currentState = Lifecycle.State.CREATED
        setDisabledShowContext(SHOW_WITH_ASSIST or SHOW_WITH_SCREENSHOT)
        current = WeakReference(this)
        scope.launch {
            model.jarvis.permissionRequest.collect { id ->
                if (id == null || !shown || permissionGeneration != null) return@collect
                permissionGeneration = id
                model.jarvis.permissionLaunched(id)
                try {
                    startAssistantActivity(Intent(context, JarvisAssistantPermissionActivity::class.java).putExtra("generation", id))
                    // Assistant windows sit above activity permission dialogs.
                    // Hide ours explicitly while retaining this generation's
                    // lease, then restore it only after the platform result.
                    hide()
                } catch (_: Exception) {
                    permissionGeneration = null
                    model.jarvis.completePermission(id, false)
                }
            }
        }
        scope.launch {
            model.jarvis.state.collect {
                cameraGeneration?.takeIf { !model.jarvis.owns(it) }?.let {
                    cameraGeneration = null
                    cameraToken = null
                    cameraOpen = false
                    _cameraBusy.value = false
                }
            }
        }
    }
    override fun onCreateContentView(): View {
        window.window?.apply {
            addFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND)
            attributes = attributes.apply { dimAmount = 0.76f }
            decorView.setViewTreeLifecycleOwner(this@JarvisAssistantSession)
            decorView.setViewTreeSavedStateRegistryOwner(this@JarvisAssistantSession)
            decorView.setViewTreeViewModelStoreOwner(this@JarvisAssistantSession)
        }
        return ComposeView(context).apply {
            setViewTreeLifecycleOwner(this@JarvisAssistantSession)
            setViewTreeSavedStateRegistryOwner(this@JarvisAssistantSession)
            setViewTreeViewModelStoreOwner(this@JarvisAssistantSession)
            setViewCompositionStrategy(ViewCompositionStrategy.DisposeOnViewTreeLifecycleDestroyed)
            setContent { JarvisAssistantOverlay(model, this@JarvisAssistantSession) }
            view = this
        }
    }
    override fun onShow(args: Bundle?, showFlags: Int) {
        trace("show")
        super.onShow(args, showFlags)
        refreshNotificationPermission()
        if (shown) return
        shown = true
        keepCall = false
        registry.currentState = Lifecycle.State.RESUMED
        if (context.getSystemService(KeyguardManager::class.java).isKeyguardLocked) {
            _notice.value = "Unlock your phone, then invoke Jarvis again."
            return
        }
        if (!VoiceInteractionService.isActiveService(context,
                android.content.ComponentName(context, JarvisAssistantService::class.java))) {
            _notice.value = "Select Zeron Jarvis as your default digital assistant."
            return
        }
        model.jarvis.showAssistant(this)
        if (cameraGeneration != null && cameraOpen) {
            scope.launch { yield(); hide() }
            return
        }
        if (permissionGeneration != null) {
            // Another hardware invocation must not cover the pending Android
            // permission dialog or create a second request.
            scope.launch { yield(); hide() }
            return
        }
        if (args?.getBoolean(RESUME) == true || model.jarvis.state.value.live) return
        startJarvis()
    }
    fun startJarvis() {
        if (!shown || !model.jarvis.assistantVisible || startJob?.isActive == true) return
        _notice.value = null
        if (!model.onboarded.value) {
            _notice.value = "Open Zeron to finish setup before using Jarvis."
            return
        }
        model.ensureBooted()
        startJob = scope.launch {
            _preparing.value = true
            try {
                // Cold invocations boot the configured runtime, wait for host
                // discovery, and retain the selected host (no silent fallback).
                withTimeoutOrNull(25_000) {
                    model.client.first { it != null }
                    model.jarvis.refreshHosts()
                    model.jarvis.hosts.first { it.isNotEmpty() }
                }
                if (shown && model.jarvis.assistantVisible) model.jarvis.requestStartFromAssistant()
            } finally { _preparing.value = false }
        }
    }
    fun dismiss() {
        keepCall = false
        permissionGeneration = null
        cameraGeneration = null
        cameraToken = null
        _cameraBusy.value = false
        model.jarvis.dismissConsent()
        model.jarvis.stop()
        hide()
    }
    private fun cancelCameraForNavigation() {
        cameraGeneration?.let { model.jarvis.pauseForCamera(it, false) }
        cameraGeneration = null
        cameraToken = null
        cameraOpen = false
        _cameraBusy.value = false
    }
    fun minimize() { cancelCameraForNavigation(); keepCall = true; hide() }
    fun capturePhoto() {
        if (!shown || permissionGeneration != null || _cameraBusy.value ||
            model.jarvis.state.value.call?.phase != uniffi.zeron_core.VoiceCallPhase.ACTIVE) return
        val id = model.jarvis.currentCallId ?: return
        cameraGeneration = id
        val token = java.util.UUID.randomUUID().toString()
        cameraToken = token
        cameraOpen = true
        _cameraBusy.value = true
        _notice.value = null
        model.jarvis.pauseForCamera(id, true)
        try {
            startAssistantActivity(Intent(context, JarvisAssistantCameraActivity::class.java)
                .putExtra("generation", id).putExtra("capture", token))
            // As with permissions, this assistant window must not cover the
            // camera app. Retain only this call's lease while capture is open.
            hide()
        } catch (_: Exception) { finishCamera(id, token, "Couldn't open the camera.") }
    }
    fun enableCallNotification() {
        if (android.os.Build.VERSION.SDK_INT < 33 || !shown || permissionGeneration != null || cameraGeneration != null) return
        val id = model.jarvis.currentCallId ?: return
        permissionGeneration = id
        try {
            startAssistantActivity(Intent(context, JarvisAssistantPermissionActivity::class.java)
                .putExtra("generation", id).putExtra("notification", true))
            hide()
        } catch (_: Exception) { permissionGeneration = null }
    }
    fun openMicrophoneSettings() {
        try {
            startAssistantActivity(Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                android.net.Uri.parse("package:${context.packageName}")))
            hide()
        } catch (_: Exception) { _notice.value = "Open Android Settings → Apps → Zeron → Permissions → Microphone." }
    }
    fun openApp(route: String) {
        cancelCameraForNavigation()
        try {
            startAssistantActivity(Intent(context, MainActivity::class.java).putExtra("route", route))
            keepCall = true
            hide()
        } catch (_: Exception) { _notice.value = "Couldn't open Zeron. Open it from your launcher." }
    }
    override fun onHide() {
        trace("hide")
        super.onHide()
        shown = false
        startJob?.cancel()
        registry.currentState = Lifecycle.State.CREATED
        // A permission activity may temporarily hide this window. Retain only
        // that in-flight generation's visibility lease until its result.
        if (permissionGeneration == null && cameraGeneration == null) model.jarvis.hideAssistant(this, keepCall)
    }
    override fun onTaskFinished(intent: Intent, taskId: Int) {
        trace("assistant task finished")
        if (intent.component?.className in listOf(JarvisAssistantPermissionActivity::class.java.name,
                JarvisAssistantCameraActivity::class.java.name)) return
        super.onTaskFinished(intent, taskId)
    }
    private fun closeSystemUi() {
        // hide() doesn't deliver onHide again when the window is already
        // hidden for a permission dialog. Revoke that lease explicitly on
        // Home/lock so a later permission result cannot open a hidden mic.
        if (shown || permissionGeneration != null || cameraGeneration != null) {
            permissionGeneration = null
            cameraGeneration = null
            cameraToken = null
            cameraOpen = false
            _cameraBusy.value = false
            keepCall = false
            startJob?.cancel()
            model.jarvis.hideAssistant(this, false)
        }
        hide()
    }
    override fun onCloseSystemDialogs() { trace("close system dialogs"); closeSystemUi() }
    override fun onLockscreenShown() { closeSystemUi() }
    override fun onDestroy() {
        cameraGeneration = null
        cameraToken = null
        permissionGeneration = null
        model.jarvis.hideAssistant(this, keepCall)
        if (current?.get() === this) current = null
        scope.cancel()
        registry.currentState = Lifecycle.State.DESTROYED
        view?.disposeComposition()
        viewModelStore.clear()
        super.onDestroy()
    }
    private fun resumePermission(id: Long) {
        trace("permission result")
        if (permissionGeneration != id) return
        permissionGeneration = null
        refreshNotificationPermission()
        if (context.getSystemService(KeyguardManager::class.java).isKeyguardLocked) {
            model.jarvis.hideAssistant(this, false)
            return
        }
        if (!shown) show(Bundle().apply { putBoolean(RESUME, true) }, 0)
    }
    private fun ownsCamera(id: Long, token: String) = cameraGeneration == id && cameraToken == token
    private fun resumeCamera(id: Long, token: String) {
        if (!ownsCamera(id, token) || !model.jarvis.owns(id)) return
        cameraOpen = false
        if (context.getSystemService(KeyguardManager::class.java).isKeyguardLocked) {
            closeSystemUi()
            return
        }
        if (!shown) show(Bundle().apply { putBoolean(RESUME, true) }, 0)
    }
    private fun finishCamera(id: Long, token: String, message: String?) {
        if (!ownsCamera(id, token)) return
        resumeCamera(id, token)
        cameraGeneration = null
        cameraToken = null
        cameraOpen = false
        _cameraBusy.value = false
        model.jarvis.pauseForCamera(id, false)
        if (!model.jarvis.owns(id)) return
        _notice.value = message
        if (message == "Photo added" || message == "Photo queued") scope.launch {
            delay(3_000)
            if (model.jarvis.owns(id) && _notice.value == message) _notice.value = null
        }
    }
    companion object {
        private const val RESUME = "sh.zeron.android.jarvis.RESUME"
        private var current: WeakReference<JarvisAssistantSession>? = null
        fun permissionFinished(id: Long) { current?.get()?.resumePermission(id) }
        fun ownsCamera(id: Long, token: String) = current?.get()?.ownsCamera(id, token) == true
        fun cameraReturned(id: Long, token: String) { current?.get()?.resumeCamera(id, token) }
        fun cameraFinished(id: Long, token: String, message: String?) { current?.get()?.finishCamera(id, token, message) }
    }
}
