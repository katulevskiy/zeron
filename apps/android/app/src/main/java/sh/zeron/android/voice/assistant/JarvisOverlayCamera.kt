package sh.zeron.android.voice.assistant

import android.content.Context
import android.util.Size
import androidx.camera.core.*
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.core.resolutionselector.ResolutionStrategy
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.core.content.ContextCompat
import androidx.lifecycle.LifecycleOwner
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.io.File
import java.util.UUID
import java.util.concurrent.Executors

internal data class OverlayCameraState(val open: Boolean = false, val ready: Boolean = false, val capturing: Boolean = false)

/** Owns only still-photo use cases. Never touches audio focus, recording, or
 * the voice lease. The TextureView preview can be clipped to a real circle. */
internal class JarvisOverlayCamera(
    private val context: Context,
    private val owner: LifecycleOwner,
    private val captured: (File) -> Unit,
    private val failed: (String) -> Unit,
) {
    private val _state = MutableStateFlow(OverlayCameraState())
    val state = _state.asStateFlow()
    private val main = ContextCompat.getMainExecutor(context)
    private val io = Executors.newSingleThreadExecutor()
    private var epoch = 0L
    private var provider: ProcessCameraProvider? = null
    private var preview: Preview? = null
    private var capture: ImageCapture? = null
    private var view: PreviewView? = null
    private var disposed = false
    private var startedAt = 0L
    private var providerFuture: com.google.common.util.concurrent.ListenableFuture<ProcessCameraProvider>? = null
    // Enumerate/configure CameraX while the user talks; no use case is bound
    // and no camera is opened until the explicit camera action.
    fun warmUp() {
        if (!disposed && providerFuture == null) providerFuture = ProcessCameraProvider.getInstance(context)
    }
    fun open() {
        if (disposed || _state.value.open) return
        startedAt = android.os.SystemClock.elapsedRealtime()
        _state.value = OverlayCameraState(open = true)
        val token = ++epoch
        io.execute {
            File(context.cacheDir, "jarvis-camera").listFiles()?.filter {
                it.lastModified() < System.currentTimeMillis() - 86_400_000
            }?.forEach { it.delete() }
        }
        warmUp()
        val future = checkNotNull(providerFuture)
        future.addListener({
            if (disposed || token != epoch || !_state.value.open) return@addListener
            try { provider = future.get(); bind() }
            catch (_: Exception) { close(); failed("Couldn't open the camera. Check camera access in Android Settings.") }
        }, main)
    }
    fun attach(next: PreviewView) {
        view = next
        next.implementationMode = PreviewView.ImplementationMode.COMPATIBLE
        next.scaleType = PreviewView.ScaleType.FILL_CENTER
        next.previewStreamState.observe(owner) { stream ->
            if (view === next && _state.value.open && stream == PreviewView.StreamState.STREAMING && !_state.value.ready) {
                _state.value = _state.value.copy(ready = true)
                trace("preview_ready", startedAt)
            }
        }
        bind()
    }
    @androidx.annotation.OptIn(markerClass = [ExperimentalZeroShutterLag::class])
    private fun bind() {
        val p = provider ?: return
        val v = view ?: return
        if (!_state.value.open || capture != null) return
        val rotation = v.display?.rotation ?: android.view.Surface.ROTATION_0
        val selector = ResolutionSelector.Builder().setResolutionStrategy(
            ResolutionStrategy(Size(1280, 960), ResolutionStrategy.FALLBACK_RULE_CLOSEST_LOWER_THEN_HIGHER)
        ).build()
        val image = ImageCapture.Builder().setCaptureMode(ImageCapture.CAPTURE_MODE_ZERO_SHUTTER_LAG)
            .setFlashMode(ImageCapture.FLASH_MODE_OFF).setResolutionSelector(selector).setTargetRotation(rotation).build()
        val stream = Preview.Builder().setTargetRotation(rotation).build()
        try {
            val camera = if (p.hasCamera(CameraSelector.DEFAULT_BACK_CAMERA)) CameraSelector.DEFAULT_BACK_CAMERA else CameraSelector.DEFAULT_FRONT_CAMERA
            p.bindToLifecycle(owner, camera, stream, image)
            preview = stream; capture = image
            stream.surfaceProvider = v.surfaceProvider
        } catch (_: Exception) { p.unbind(stream, image); close(); failed("Camera unavailable. Your voice call is still active.") }
    }
    fun shutter() {
        val image = capture ?: return
        if (!_state.value.ready || _state.value.capturing) return
        _state.value = _state.value.copy(capturing = true)
        val token = epoch
        val started = android.os.SystemClock.elapsedRealtime()
        image.targetRotation = view?.display?.rotation ?: image.targetRotation
        val folder = File(context.cacheDir, "jarvis-camera").apply { mkdirs() }
        val file = File(folder, "photo-${UUID.randomUUID()}.jpg")
        image.takePicture(ImageCapture.OutputFileOptions.Builder(file).build(), io, object : ImageCapture.OnImageSavedCallback {
            override fun onImageSaved(result: ImageCapture.OutputFileResults) { main.execute {
                if (disposed || token != epoch || !_state.value.open) { file.delete(); return@execute }
                trace("capture_saved", started)
                // Return to the live orb immediately. Encoding/transfer belongs
                // to the session's async job, not the camera or an activity.
                close()
                captured(file)
            } }
            override fun onError(error: ImageCaptureException) { file.delete(); main.execute {
                if (token != epoch || disposed) return@execute
                _state.value = _state.value.copy(capturing = false)
                failed("Couldn't take the photo. Try the shutter again.")
            } }
        })
    }
    fun close() {
        ++epoch
        preview?.let { provider?.unbind(it) }; capture?.let { provider?.unbind(it) }
        preview = null; capture = null
        view?.previewStreamState?.removeObservers(owner)
        view = null
        _state.value = OverlayCameraState()
    }
    fun dispose() { disposed = true; close(); io.shutdown() }
    private fun trace(stage: String, since: Long) {
        if (context.applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE != 0)
            android.util.Log.d("JarvisCamera", "$stage elapsed_ms=${android.os.SystemClock.elapsedRealtime() - since}")
    }
}
