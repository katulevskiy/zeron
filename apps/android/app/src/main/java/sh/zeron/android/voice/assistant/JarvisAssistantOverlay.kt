package sh.zeron.android.voice.assistant

import androidx.compose.foundation.background
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.*
import androidx.compose.material.icons.automirrored.filled.VolumeUp
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.ZeronTheme
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.voice.JarvisOrb
import uniffi.zeron_core.*

@Composable
fun JarvisAssistantOverlay(model: AppModel, session: JarvisAssistantSession) {
    val onboarded by model.onboarded.collectAsState()
    val controller = model.jarvis
    val state by controller.state.collectAsState()
    val consent by controller.consentRequest.collectAsState()
    val preparing by session.preparing.collectAsState()
    val notice by session.notice.collectAsState()
    val notifications by session.notifications.collectAsState()
    val blurred by session.blurred.collectAsState()
    // Assistant chrome follows the PHONE theme, independently of the app's
    // appearance setting. The system compositor blurs the other app, not this UI.
    ZeronTheme {
        val dark = LocalDarkTheme.current
        // Text actions/errors need stronger contrast than the app's opaque
        // palette when arbitrary content shows through the glass.
        val scheme = MaterialTheme.colorScheme.copy(
            primary = if (dark) Color(0xFFC4B5FD) else Color(0xFF4525BE),
            onPrimary = if (dark) Color(0xFF26105C) else Color.White,
            error = if (dark) Color(0xFFFF9C9C) else Color(0xFF991414),
        )
        MaterialTheme(colorScheme = scheme) {
            val glass = if (dark) Color(0xFF111116) else Color(0xFFFAFAFD)
            val opacity = if (blurred) 0.78f else 0.90f
            BoxWithConstraints(Modifier.fillMaxSize().systemBarsPadding()) {
                Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = if (blurred) 0.08f else 0.16f))
                    .clickable(onClickLabel = "Dismiss Jarvis", onClick = session::dismiss))
                Surface(shape = RoundedCornerShape(28.dp), color = glass.copy(alpha = opacity),
                    contentColor = MaterialTheme.colorScheme.onSurface,
                    // Elevation tint would change the deliberately translucent glass.
                    tonalElevation = 0.dp, shadowElevation = 0.dp,
                    border = BorderStroke(1.dp, Color.White.copy(alpha = if (dark) 0.14f else 0.50f)),
                    modifier = Modifier.align(Alignment.BottomCenter).padding(12.dp)
                        .widthIn(max = 600.dp).fillMaxWidth().heightIn(max = maxHeight - 24.dp)
                        .semantics { paneTitle = "Zeron Jarvis assistant" }) {
                    Column(Modifier.verticalScroll(rememberScrollState()).padding(20.dp),
                        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                            Text("Jarvis", style = MaterialTheme.typography.titleLarge, modifier = Modifier.weight(1f))
                            IconButton(onClick = { session.openApp("jarvis-settings") }) { Icon(Icons.Default.Settings, "Jarvis settings") }
                            IconButton(onClick = session::dismiss) { Icon(Icons.Default.Close, "Close Jarvis and end call") }
                        }
                        if (consent != null) {
                            Text("Your phone sends live microphone audio to OpenAI. Codex on ${consent!!.name} runs Jarvis, uses your ChatGPT voice allowance, and saves the conversation in its Zeron chat. Jarvis can use your connected agents and tools. You can mute or hang up at any time.")
                            Button(onClick = controller::acceptConsent) { Text("Continue") }
                            TextButton(onClick = session::dismiss) { Text("Cancel") }
                        } else {
                            JarvisOrb(if (state.muted) VoiceOrb.MUTED else state.call?.orb ?: VoiceOrb.IDLE,
                                state.call?.microphone ?: 0f, state.call?.speaker ?: 0f, Modifier.size(136.dp))
                            val status = when {
                                preparing -> "Preparing Jarvis…"
                                state.muted -> "Microphone muted"
                                state.call?.work == VoiceCallWork.AWAITING_INPUT -> "Needs your input"
                                state.call?.work == VoiceCallWork.WORKING -> "Working"
                                state.call?.speaking == true -> "Speaking"
                                state.call?.phase == VoiceCallPhase.ACTIVE -> "Listening"
                                state.live -> "Connecting…"
                                else -> "Ready when you are"
                            }
                            Text(status, style = MaterialTheme.typography.headlineSmall,
                                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
                            if (state.host.isNotBlank()) Text(state.host, style = MaterialTheme.typography.bodySmall)
                            (notice ?: state.error)?.let { Text(it, color = MaterialTheme.colorScheme.error,
                                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite }) }
                            state.call?.caption?.takeLast(240)?.takeIf { it.isNotBlank() }?.let { Text(it) }
                            if (state.live) {
                                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                    FilledTonalIconButton(onClick = controller::toggleMute, modifier = Modifier.size(56.dp)) {
                                        Icon(if (state.muted) Icons.Default.MicOff else Icons.Default.Mic,
                                            if (state.muted) "Unmute microphone" else "Mute microphone")
                                    }
                                    FilledTonalIconButton(onClick = controller::toggleSpeaker, modifier = Modifier.size(56.dp)) {
                                        Icon(if (state.speaker) Icons.AutoMirrored.Filled.VolumeUp else Icons.Default.PhoneInTalk,
                                            if (state.speaker) "Use earpiece" else "Use speaker")
                                    }
                                    FilledIconButton(onClick = session::dismiss, modifier = Modifier.size(56.dp),
                                        colors = IconButtonDefaults.filledIconButtonColors(
                                            containerColor = MaterialTheme.colorScheme.error,
                                            contentColor = MaterialTheme.colorScheme.onError)) {
                                        Icon(Icons.Default.CallEnd, "Hang up")
                                    }
                                }
                                if (!notifications) TextButton(onClick = session::enableCallNotification) { Text("Enable call notification") }
                                TextButton(onClick = session::minimize) { Text("Keep call in background") }
                                state.call?.chatId?.let { id -> TextButton(onClick = { session.openApp("chat:$id") }) { Text("Open conversation") } }
                                Text(if (notifications) "Closing this overlay ends the call. Background calls stay in the call notification." else "Closing this overlay ends the call. Enable notifications for background mute and hang-up controls.", style = MaterialTheme.typography.bodySmall)
                            } else if (!preparing) {
                                if (notice == null) Button(onClick = session::startJarvis) { Text("Start Jarvis") }
                                if (state.endReason == VoiceEndReason.MICROPHONE_DENIED) TextButton(onClick = session::openMicrophoneSettings) { Text("Microphone permission settings") }
                                if (state.endReason in listOf(VoiceEndReason.SIGN_IN_REQUIRED, VoiceEndReason.HOST_INCOMPATIBLE)) {
                                    TextButton(onClick = { session.openApp("agents") }) { Text("Set up Codex") }
                                }
                                TextButton(onClick = { session.openApp(if (onboarded) "jarvis-settings" else "home") }) {
                                    Text(if (onboarded) "Choose device and voice" else "Open Zeron")
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
