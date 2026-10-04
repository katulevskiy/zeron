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
    private val photoJobs = mutableMapOf<String, Job>()
    private val _uploading = MutableStateFlow(0)
    val uploading = _uploading.asStateFlow()
    internal val camera = JarvisOverlayCamera(context, this, ::deliverPhoto, ::showNotice)
    private var cameraPermission = false
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
        updateScreenContext()
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
                if (shown && it.call?.phase == uniffi.zeron_core.VoiceCallPhase.ACTIVE) camera.warmUp()
                if (!it.live) {
                    camera.close()
                    photoJobs.values.toList().forEach { job -> job.cancel() }
                    photoJobs.clear()
                    _uploading.value = 0
                    // A stale success must never hide recovery/start controls
                    // after a call has closed, even during an activity handoff.
                    if (_notice.value?.startsWith("Photo ") == true) _notice.value = null
                }
            }
        }
    }
    private fun updateScreenContext() {
        setDisabledShowContext(SHOW_WITH_ASSIST or if (model.jarvis.screen.settings.contextEnabled.value) 0 else SHOW_WITH_SCREENSHOT)
    }
    override fun onHandleScreenshot(screenshot: android.graphics.Bitmap?) { model.jarvis.screen.acceptAssist(screenshot) }
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
        updateScreenContext()
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
        if (context.getSystemService(KeyguardManager::class.java).isKeyguardLocked) return
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
        cameraPermission = false
        cancelCameraForNavigation()
        model.jarvis.dismissConsent()
        model.jarvis.stop()
        hide()
    }
    private fun cancelCameraForNavigation() {
        camera.close()
        photoJobs.values.toList().forEach { it.cancel() }
        photoJobs.clear()
        _uploading.value = 0
    }
    fun minimize() { cancelCameraForNavigation(); keepCall = true; hide() }
    fun capturePhoto() {
        if (!shown || permissionGeneration != null || photoJobs.size >= 3 ||
            model.jarvis.state.value.call?.phase != uniffi.zeron_core.VoiceCallPhase.ACTIVE) return
        if (camera.state.value.open) { camera.close(); return }
        _notice.value = null
        if (androidx.core.content.ContextCompat.checkSelfPermission(context, android.Manifest.permission.CAMERA) == android.content.pm.PackageManager.PERMISSION_GRANTED) {
            camera.open()
            return
        }
        val id = model.jarvis.currentCallId ?: return
        permissionGeneration = id
        cameraPermission = true
        try {
            startAssistantActivity(Intent(context, JarvisAssistantPermissionActivity::class.java)
                .putExtra("generation", id).putExtra("camera", true))
            // Only the one-time platform permission dialog needs a bridge;
            // the call and its microphone keep their existing lease.
            hide()
        } catch (_: Exception) {
            permissionGeneration = null; cameraPermission = false
            showNotice("Allow camera access in Android Settings to take a photo.")
        }
    }
    fun takePhoto() { if (shown && model.jarvis.state.value.live) camera.shutter() }
    fun closeCamera() { camera.close() }
    private fun deliverPhoto(file: java.io.File) {
        val id = model.jarvis.currentCallId
        if (!shown || id == null || photoJobs.size >= 3) { file.delete(); return }
        val token = java.util.UUID.randomUUID().toString()
        val job = scope.launch(start = CoroutineStart.LAZY) {
            try {
                val result = model.jarvis.addPhoto(id, file) { shown && photoJobs.containsKey(token) }
                if (model.jarvis.owns(id) && shown) showNotice(result)
            } catch (e: CancellationException) { throw e }
            catch (e: sh.zeron.android.voice.VoicePhotoException) {
                if (model.jarvis.owns(id) && shown) showNotice(e.reason.message)
            } catch (_: Exception) {
                if (model.jarvis.owns(id) && shown) showNotice("Couldn't add the photo. Your voice call is still active.")
            } finally {
                file.delete()
                photoJobs.remove(token)
                _uploading.value = photoJobs.size
            }
        }
        photoJobs[token] = job
        _uploading.value = photoJobs.size
        job.start()
    }
    private var noticeJob: Job? = null
    private fun showNotice(message: String) {
        noticeJob?.cancel()
        _notice.value = message
        if (message == "Photo added" || message == "Photo queued") noticeJob = scope.launch {
            delay(2_000)
            if (_notice.value == message) _notice.value = null
        }
    }
    fun enableCallNotification() {
        if (android.os.Build.VERSION.SDK_INT < 33 || !shown || permissionGeneration != null) return
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
        if (permissionGeneration == null) cancelCameraForNavigation()
        // A permission activity may temporarily hide this window. Retain only
        // that in-flight generation's visibility lease until its result.
        if (permissionGeneration == null) model.jarvis.hideAssistant(this, keepCall)
    }
    override fun onTaskFinished(intent: Intent, taskId: Int) {
        trace("assistant task finished")
        if (intent.component?.className == JarvisAssistantPermissionActivity::class.java.name) return
        super.onTaskFinished(intent, taskId)
    }
    private fun closeSystemUi() {
        // hide() doesn't deliver onHide again when the window is already
        // hidden for a permission dialog. Revoke that lease explicitly on
        // Home/lock so a later permission result cannot open a hidden mic.
        if (shown || permissionGeneration != null) {
            permissionGeneration = null
            cameraPermission = false
            cancelCameraForNavigation()
            keepCall = false
            startJob?.cancel()
            model.jarvis.hideAssistant(this, false)
        }
        hide()
    }
    override fun onCloseSystemDialogs() { trace("close system dialogs"); closeSystemUi() }
    override fun onBackPressed() { if (camera.state.value.open) camera.close() else dismiss() }
    override fun onLockscreenShown() { model.jarvis.screen.stop(); closeSystemUi() }
    override fun onDestroy() {
        camera.dispose()
        cancelCameraForNavigation()
        permissionGeneration = null
        model.jarvis.hideAssistant(this, keepCall)
        if (current?.get() === this) current = null
        scope.cancel()
        registry.currentState = Lifecycle.State.DESTROYED
        view?.disposeComposition()
        viewModelStore.clear()
        super.onDestroy()
    }
    private fun resumePermission(id: Long, granted: Boolean) {
        trace("permission result")
        if (permissionGeneration != id) return
        val wasCamera = cameraPermission
        cameraPermission = false
        permissionGeneration = null
        refreshNotificationPermission()
        if (context.getSystemService(KeyguardManager::class.java).isKeyguardLocked) {
            model.jarvis.hideAssistant(this, false)
            return
        }
        if (!shown) show(Bundle().apply { putBoolean(RESUME, true) }, 0)
        if (wasCamera && model.jarvis.owns(id)) {
            if (granted && shown) camera.open()
            else showNotice("Camera access denied. You can keep talking to Jarvis.")
        }
    }
    companion object {
        private const val RESUME = "sh.zeron.android.jarvis.RESUME"
        private var current: WeakReference<JarvisAssistantSession>? = null
        fun refreshContext() { current?.get()?.updateScreenContext() }
        fun hideForControl() { current?.get()?.takeIf { it.shown }?.minimize() }
        fun permissionFinished(id: Long, granted: Boolean = false) { current?.get()?.resumePermission(id, granted) }
    }
}
