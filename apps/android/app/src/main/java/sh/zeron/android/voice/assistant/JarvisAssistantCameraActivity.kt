package sh.zeron.android.voice.assistant

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.core.content.FileProvider
import androidx.lifecycle.ViewModel
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import sh.zeron.android.ZeronApplication
import sh.zeron.android.voice.JarvisController
import sh.zeron.android.voice.VoicePhotoException
import java.io.File
import java.util.UUID

/** The user's camera app owns capture. No camera/storage permission or gallery
 * copy is needed. Only a private, temporary photo URI is granted to that app. */
class JarvisAssistantCameraActivity : ComponentActivity() {
    private var generation = 0L
    private var capture = ""
    private lateinit var file: File
    private lateinit var uri: Uri
    private val delivery by viewModels<JarvisPhotoDelivery>()
    private val camera = registerForActivityResult(JarvisCameraContract()) { captured ->
        JarvisAssistantSession.cameraReturned(generation, capture)
        if (captured) {
            val controller = (application as ZeronApplication).existingModel?.jarvis
            if (controller?.owns(generation) == true && JarvisAssistantSession.ownsCamera(generation, capture)) delivery.send(controller, generation, capture, file)
            else finish()
        } else {
            JarvisAssistantSession.cameraFinished(generation, capture, null)
            finish()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        generation = intent.getLongExtra("generation", 0)
        capture = intent.getStringExtra("capture").orEmpty()
        val controller = (application as ZeronApplication).existingModel?.jarvis
        if (controller?.owns(generation) != true || !controller.assistantVisible ||
            !JarvisAssistantSession.ownsCamera(generation, capture)) { finish(); return }
        val folder = File(cacheDir, "jarvis-camera").apply { mkdirs() }
        folder.listFiles()?.filter { it.lastModified() < System.currentTimeMillis() - 86_400_000 }?.forEach { it.delete() }
        file = File(folder, savedInstanceState?.getString("photo") ?: "photo-${UUID.randomUUID()}.jpg")
        uri = FileProvider.getUriForFile(this, "$packageName.files", file)
        lifecycleScope.launch {
            combine(controller.state, controller.assistantPresentation) { _, visible ->
                controller.owns(generation) && visible
            }.collect { owned ->
                if (!owned) finishAndRemoveTask()
            }
        }
        lifecycleScope.launch {
            delivery.result.collect { result ->
                result?.let {
                    JarvisAssistantSession.cameraFinished(generation, capture, it)
                    finish()
                }
            }
        }
        if (savedInstanceState == null) {
            try { camera.launch(uri) }
            catch (_: Exception) {
                JarvisAssistantSession.cameraFinished(generation, capture, "Couldn't open the camera. Check that a camera app is installed.")
                finish()
            }
        }
    }
    override fun onSaveInstanceState(outState: Bundle) {
        if (::file.isInitialized) outState.putString("photo", file.name)
        super.onSaveInstanceState(outState)
    }
    override fun onDestroy() {
        if (!isChangingConfigurations && ::file.isInitialized) {
            JarvisAssistantSession.cameraFinished(generation, capture, null)
            revokeUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
            file.delete()
        }
        super.onDestroy()
    }
}

internal class JarvisCameraContract : ActivityResultContracts.TakePicture() {
    override fun createIntent(context: Context, input: Uri): Intent = super.createIntent(context, input).apply {
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
        clipData = ClipData.newRawUri("Jarvis photo", input)
    }
}

/** Encoding/delivery survives camera-bridge rotation and never sends twice. */
class JarvisPhotoDelivery : ViewModel() {
    val result = MutableStateFlow<String?>(null)
    private var started = false
    internal fun send(controller: JarvisController, id: Long, capture: String, file: File) {
        if (started) return
        started = true
        viewModelScope.launch {
            result.value = try {
                controller.addPhoto(id, file) { JarvisAssistantSession.ownsCamera(id, capture) }
            } catch (_: CancellationException) {
                currentCoroutineContext().ensureActive()
                "Photo pending — check the conversation before retrying."
            }
            catch (e: VoicePhotoException) {
                // Only log our bounded classification, never a provider error
                // body, camera path, image bytes, or credentials.
                android.util.Log.w("JarvisPhoto", "Photo delivery failed: ${e.reason.name}")
                e.reason.message
            }
            catch (_: Exception) { "Couldn't add the photo to this call. Try again." }
        }
    }
}
