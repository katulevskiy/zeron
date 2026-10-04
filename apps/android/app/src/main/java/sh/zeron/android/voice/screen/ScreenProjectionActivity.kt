package sh.zeron.android.voice.screen

import android.content.Intent
import android.media.projection.MediaProjectionManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import sh.zeron.android.ZeronApplication

/** Android's consent UI is requested separately for every sharing session. */
class ScreenProjectionActivity : ComponentActivity() {
    private var generation = 0L
    private val consent = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        val controller = (application as ZeronApplication).existingModel?.jarvis
        if (result.resultCode == RESULT_OK && result.data != null && controller?.owns(generation) == true && controller.screen.settings.contextEnabled.value) {
            try { ContextCompat.startForegroundService(this, Intent(this, ScreenProjectionService::class.java)
                .putExtra("generation", generation).putExtra("result", result.resultCode).putExtra("consent", result.data)) }
            catch (_: Exception) { controller.screen.stopSharing() }
        } else controller?.screen?.report("Screen sharing cancelled")
        finish()
    }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        generation = savedInstanceState?.getLong("generation") ?: (application as ZeronApplication).existingModel?.jarvis?.currentCallId ?: 0L
        if ((application as ZeronApplication).existingModel?.jarvis?.owns(generation) != true) { finish(); return }
        if (savedInstanceState == null) consent.launch(getSystemService(MediaProjectionManager::class.java).createScreenCaptureIntent())
    }
    override fun onSaveInstanceState(outState: Bundle) { outState.putLong("generation", generation); super.onSaveInstanceState(outState) }
}
