package sh.zeron.android.voice.assistant

import android.app.role.RoleManager
import android.content.Intent
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.LifecycleResumeEffect

@Composable
fun JarvisAssistantSetup() {
    val context = LocalContext.current
    val roles = remember { context.getSystemService(RoleManager::class.java) }
    var selected by remember { mutableStateOf(roles.isRoleHeld(RoleManager.ROLE_ASSISTANT)) }
    var error by remember { mutableStateOf<String?>(null) }
    LifecycleResumeEffect(Unit) {
        selected = roles.isRoleHeld(RoleManager.ROLE_ASSISTANT)
        onPauseOrDispose { }
    }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        selected = roles.isRoleHeld(RoleManager.ROLE_ASSISTANT)
    }
    fun launch(intent: Intent) {
        try { launcher.launch(intent); error = null }
        catch (_: android.content.ActivityNotFoundException) { error = "Open Android Settings → Apps → Default apps → Digital assistant app and select Zeron Jarvis." }
    }
    Text("Android assistant", style = MaterialTheme.typography.titleLarge)
    Text(if (selected) "Zeron is your default digital assistant." else "Start Jarvis over any app using your phone's assistant gesture.")
    Button(onClick = {
        // ASSISTANT is not requestable through createRequestRoleIntent on
        // AOSP/Pixel. The voice-input settings screen owns its system picker.
        launch(Intent(Settings.ACTION_VOICE_INPUT_SETTINGS))
    }) { Text(if (selected) "Default assistant settings" else "Use Zeron as default assistant") }
    Text("Then set press and hold power button to Digital assistant in your phone's gesture or side-button settings. You can also use the corner-swipe or hold-home assistant gesture where supported. The exact gesture depends on your phone.")
    TextButton(onClick = { launch(Intent(Settings.ACTION_SETTINGS)) }) { Text("Open phone gesture settings") }
    Text("Jarvis opens only when invoked. It doesn't listen for a wake word or read the screen underneath. Unlock your phone before starting a call.", style = MaterialTheme.typography.bodySmall)
    error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    HorizontalDivider()
}
