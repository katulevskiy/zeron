package sh.zeron.android.voice.assistant

import android.Manifest
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import sh.zeron.android.ZeronApplication
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch

/** A translucent, non-exported activity provides Android's runtime permission
 * UI without bringing the Zeron task to the front. Results retain their call
 * generation across rotation and cannot revive a cancelled session. */
class JarvisAssistantPermissionActivity : ComponentActivity() {
    private var generation = 0L
    private val permission = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (!intent.getBooleanExtra("notification", false) && !intent.getBooleanExtra("camera", false))
            (application as ZeronApplication).existingModel?.jarvis?.completePermission(generation, granted)
        JarvisAssistantSession.permissionFinished(generation, granted)
        finish()
    }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        generation = intent.getLongExtra("generation", 0)
        val controller = (application as ZeronApplication).existingModel?.jarvis
        if (controller?.owns(generation) != true || !controller.assistantVisible) {
            JarvisAssistantSession.permissionFinished(generation)
            finish()
            return
        }
        lifecycleScope.launch {
            controller.state.collect {
                if (!controller.owns(generation)) {
                    JarvisAssistantSession.permissionFinished(generation)
                    finishAndRemoveTask()
                }
            }
        }
        if (savedInstanceState == null) permission.launch(
            if (intent.getBooleanExtra("notification", false) && android.os.Build.VERSION.SDK_INT >= 33)
                Manifest.permission.POST_NOTIFICATIONS else if (intent.getBooleanExtra("camera", false)) Manifest.permission.CAMERA else Manifest.permission.RECORD_AUDIO)
    }
}

/** The system's assistant preferences entry opens the same settings page. */
class JarvisAssistantSettingsActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        startActivity(android.content.Intent(this, sh.zeron.android.MainActivity::class.java).putExtra("route", "jarvis-settings"))
        finish()
    }
}
