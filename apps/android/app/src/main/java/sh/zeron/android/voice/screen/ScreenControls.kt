package sh.zeron.android.voice.screen

import android.content.Intent
import android.provider.Settings
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import sh.zeron.android.voice.JarvisController

@Composable
fun JarvisScreenSettings(controller: JarvisController) {
    val screen = controller.screen
    val context = LocalContext.current
    val enabled by screen.settings.contextEnabled.collectAsState()
    val control by screen.settings.controlEnabled.collectAsState()
    val hasKey by screen.settings.hasKey.collectAsState()
    val state by screen.state.collectAsState()
    var editing by remember { mutableStateOf(false) }
    Text("Phone screen · experimental", style = MaterialTheme.typography.titleLarge)
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
        Text("Screen context", Modifier.weight(1f)); Switch(enabled, screen::setContext)
    }
    Text("Share a screenshot on request, or start live sharing for questions as the screen changes. Screen images go to your Jarvis model.", style = MaterialTheme.typography.bodySmall)
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
        Text("Phone control", Modifier.weight(1f)); Switch(control, screen::setControl)
    }
    Text("Let Jarvis use Jev to choose visible controls for your requested tasks. Visible screen text is sent to TypeSafe. Choose This phone as the execution device for phone control.", style = MaterialTheme.typography.bodySmall)
    OutlinedButton(onClick = { context.startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)) }) {
        Text("Enable Jarvis in Android Accessibility")
    }
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedButton(onClick = { editing = true }) { Text(if (hasKey) "Replace Jev key" else "Add Jev key") }
        if (hasKey) {
            TextButton(onClick = screen::checkKey) { Text("Test key") }
            TextButton(onClick = { screen.stopControl(); screen.settings.clearKey() }) { Text("Remove") }
        }
    }
    state.message?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
    val call by controller.state.collectAsState()
    if (call.live) ScreenSessionControls(controller)
    if (editing) {
        var key by remember { mutableStateOf("") }
        var failed by remember { mutableStateOf(false) }
        AlertDialog(properties = androidx.compose.ui.window.DialogProperties(securePolicy = androidx.compose.ui.window.SecureFlagPolicy.SecureOn), onDismissRequest = { key = ""; editing = false }, title = { Text("Jev API key") },
            text = {
                Column {
                    Text("Stored encrypted on this phone. The key is never sent to Jarvis or included in builds.")
                    OutlinedTextField(key, { key = it; failed = false }, label = { Text("API key") },
                        visualTransformation = PasswordVisualTransformation(), singleLine = true, isError = failed,
                        keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(keyboardType = androidx.compose.ui.text.input.KeyboardType.Password, autoCorrectEnabled = false))
                    if (failed) Text("Couldn't save this key. Check the value and try again.")
                }
            }, confirmButton = { TextButton(onClick = {
                try { screen.settings.saveKey(key); key = ""; editing = false }
                catch (_: Exception) { failed = true }
            }, enabled = key.isNotBlank()) { Text("Save") } },
            dismissButton = { TextButton(onClick = { key = ""; editing = false }) { Text("Cancel") } })
    }
}

@Composable
fun ScreenSessionControls(controller: JarvisController) {
    val screen = controller.screen
    val state by screen.state.collectAsState()
    val context = LocalContext.current
    val enabled by screen.settings.contextEnabled.collectAsState()
    val control by screen.settings.controlEnabled.collectAsState()
    if (enabled) {
        OutlinedButton(onClick = screen::shareOnce) { Text("Share screenshot") }
        OutlinedButton(onClick = { if (state.sharing) screen.stopSharing() else screen.launchProjection(context) }) {
            Text(if (state.sharing) "Stop screen sharing" else "Start live screen sharing")
        }
    }
    if (control) {
        OutlinedButton(onClick = { if (state.controlling) screen.stopControl() else screen.startControl() }) {
            Text(if (state.controlling) "Stop phone control" else "Enable control for this call")
        }
        if (state.controlling) {
            var goal by remember { mutableStateOf("") }
            var text by remember { mutableStateOf("") }
            OutlinedTextField(goal, { goal = it.take(2000) }, label = { Text("Try a task, e.g. open Display settings") }, modifier = Modifier.fillMaxWidth())
            OutlinedTextField(text, { text = it.take(2000) }, label = { Text("Exact text to type (optional)") }, modifier = Modifier.fillMaxWidth())
            TextButton(onClick = { var activityContext: android.content.Context? = context
                while (activityContext is android.content.ContextWrapper && activityContext !is android.app.Activity) activityContext = activityContext.baseContext
                (activityContext as? android.app.Activity)?.moveTaskToBack(true)
                screen.runGoal(goal, text.takeIf(String::isNotBlank)) }, enabled = goal.isNotBlank()) { Text("Try with Jev") }
            Text("Or tell Jarvis what to do by voice. Control continues when the overlay hides.", style = MaterialTheme.typography.bodySmall)
        }
    }
}

@Composable
fun ScreenControlsDialog(controller: JarvisController, onDismiss: () -> Unit) {
    val state by controller.screen.state.collectAsState()
    AlertDialog(onDismissRequest = onDismiss, title = { Text("Phone screen") },
        text = { Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            val enabled by controller.screen.settings.contextEnabled.collectAsState()
            val control by controller.screen.settings.controlEnabled.collectAsState()
            if (!enabled && !control) Text("Enable screen context or phone control in Jarvis settings first.")
            ScreenSessionControls(controller)
            state.message?.let { Text(it) }
        } }, confirmButton = { TextButton(onClick = onDismiss) { Text("Done") } })
}
