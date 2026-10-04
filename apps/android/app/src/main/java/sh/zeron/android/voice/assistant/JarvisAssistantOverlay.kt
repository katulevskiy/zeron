package sh.zeron.android.voice.assistant

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.VolumeUp
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.Appearance
import sh.zeron.android.design.ThemeMode
import sh.zeron.android.design.ZeronTheme
import sh.zeron.android.voice.JarvisOrb
import sh.zeron.android.voice.JarvisState
import sh.zeron.android.voice.VoiceHost
import uniffi.zeron_core.*

internal enum class AssistantAction { Close, Settings, Start, Consent, Mute, Speaker, Minimize, Camera, MicrophoneSettings, CodexSettings, Notification, Conversation, Setup }

@Composable
fun JarvisAssistantOverlay(model: AppModel, session: JarvisAssistantSession) {
    val controller = model.jarvis
    val state by controller.state.collectAsState()
    val consent by controller.consentRequest.collectAsState()
    val preparing by session.preparing.collectAsState()
    val notice by session.notice.collectAsState()
    val notifications by session.notifications.collectAsState()
    val cameraBusy by session.cameraBusy.collectAsState()
    JarvisAssistantContent(state, preparing, notice, consent, cameraBusy, notifications) { action ->
        when (action) {
            AssistantAction.Close -> session.dismiss()
            AssistantAction.Settings -> session.openApp("jarvis-settings")
            AssistantAction.Start -> session.startJarvis()
            AssistantAction.Consent -> controller.acceptConsent()
            AssistantAction.Mute -> controller.toggleMute()
            AssistantAction.Speaker -> controller.toggleSpeaker()
            AssistantAction.Minimize -> session.minimize()
            AssistantAction.Camera -> session.capturePhoto()
            AssistantAction.MicrophoneSettings -> session.openMicrophoneSettings()
            AssistantAction.CodexSettings -> session.openApp("agents")
            AssistantAction.Notification -> session.enableCallNotification()
            AssistantAction.Conversation -> state.call?.chatId?.let { session.openApp("chat:$it") }
            AssistantAction.Setup -> session.openApp(if (model.onboarded.value) "jarvis-settings" else "home")
        }
    }
}

/** Transparent assistant content over Android's full-screen dim layer. No card,
 * blur, title, host label, or instructional footer covers the current app. */
@Composable
internal fun JarvisAssistantContent(
    state: JarvisState,
    preparing: Boolean,
    notice: String?,
    consent: VoiceHost?,
    cameraBusy: Boolean,
    notifications: Boolean,
    onAction: (AssistantAction) -> Unit,
) {
    ZeronTheme(Appearance(ThemeMode.Dark)) {
        val scheme = MaterialTheme.colorScheme.copy(primary = Color(0xFFC4B5FD),
            onPrimary = Color(0xFF26105C), error = Color(0xFFFF9C9C))
        MaterialTheme(colorScheme = scheme) {
            BoxWithConstraints(Modifier.fillMaxSize().systemBarsPadding().semantics { paneTitle = "Zeron Jarvis assistant" }) {
                Box(Modifier.fillMaxSize().clickable(onClickLabel = "Dismiss Jarvis", onClick = { onAction(AssistantAction.Close) }))
                Column(Modifier.align(Alignment.BottomCenter).widthIn(max = 600.dp).fillMaxWidth()
                    .heightIn(max = maxHeight).verticalScroll(rememberScrollState()).padding(horizontal = 28.dp, vertical = 24.dp),
                    horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    if (consent != null) {
                        Text("Your phone sends live microphone audio to OpenAI. Codex on ${consent.name} runs Jarvis, uses your ChatGPT voice allowance, and saves the conversation in its Zeron chat. Jarvis can use your connected agents and tools. You can mute or hang up at any time.", color = scheme.onSurface)
                        Button(onClick = { onAction(AssistantAction.Consent) }) { Text("Continue") }
                        TextButton(onClick = { onAction(AssistantAction.Close) }) { Text("Cancel") }
                    } else {
                        JarvisOrb(if (state.muted) VoiceOrb.MUTED else state.call?.orb ?: VoiceOrb.IDLE,
                            state.call?.microphone ?: 0f, state.call?.speaker ?: 0f, Modifier.size(184.dp),
                            tint = Color(0xFF8B7CF6))
                        val status = when {
                            cameraBusy -> "Adding photo…"
                            preparing -> "Preparing…"
                            state.muted -> "Microphone muted"
                            state.call?.work == VoiceCallWork.AWAITING_INPUT -> "Needs your input"
                            state.call?.work == VoiceCallWork.WORKING -> "Working"
                            state.live && state.call?.phase != VoiceCallPhase.ACTIVE -> "Connecting…"
                            else -> null
                        }
                        status?.let { Text(it, color = scheme.onSurface, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite }) }
                        (notice ?: state.error)?.let { Text(it, color = if (notice == "Photo added") scheme.onSurface else scheme.error,
                            modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite }) }
                        state.call?.caption?.takeLast(240)?.takeIf { it.isNotBlank() }?.let {
                            Text(it, color = scheme.onSurface, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
                        }
                        if (state.live) {
                            Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                                FilledTonalIconButton(onClick = { onAction(AssistantAction.Mute) }, modifier = Modifier.size(56.dp)) {
                                    Icon(if (state.muted) Icons.Default.MicOff else Icons.Default.Mic,
                                        if (state.muted) "Unmute microphone" else "Mute microphone")
                                }
                                FilledTonalIconButton(onClick = { onAction(AssistantAction.Speaker) }, modifier = Modifier.size(56.dp)) {
                                    Icon(if (state.speaker) Icons.AutoMirrored.Filled.VolumeUp else Icons.Default.PhoneInTalk,
                                        if (state.speaker) "Use earpiece" else "Use speaker")
                                }
                                FilledIconButton(onClick = { onAction(AssistantAction.Close) }, modifier = Modifier.size(56.dp),
                                    colors = IconButtonDefaults.filledIconButtonColors(containerColor = scheme.error, contentColor = scheme.onError)) {
                                    Icon(Icons.Default.CallEnd, "Hang up")
                                }
                            }
                            if (!notifications) TextButton(onClick = { onAction(AssistantAction.Notification) }) { Text("Enable call notification") }
                            if (state.call?.work == VoiceCallWork.AWAITING_INPUT && state.call.chatId != null) {
                                TextButton(onClick = { onAction(AssistantAction.Conversation) }) { Text("Open conversation") }
                            }
                        } else if (!preparing) {
                            if (notice == null) Button(onClick = { onAction(AssistantAction.Start) }) { Text("Start Jarvis") }
                            if (state.endReason == VoiceEndReason.MICROPHONE_DENIED) TextButton(onClick = { onAction(AssistantAction.MicrophoneSettings) }) { Text("Microphone permissions") }
                            if (state.endReason in listOf(VoiceEndReason.SIGN_IN_REQUIRED, VoiceEndReason.HOST_INCOMPATIBLE)) {
                                TextButton(onClick = { onAction(AssistantAction.CodexSettings) }) { Text("Set up Codex") }
                            }
                            if (state.error != null && state.endReason == null) TextButton(onClick = { onAction(AssistantAction.Setup) }) { Text("Open Zeron") }
                        }
                    }
                    Row(Modifier.fillMaxWidth().padding(top = 8.dp), horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically) {
                        FilledTonalIconButton(onClick = { onAction(AssistantAction.Settings) }, modifier = Modifier.size(48.dp)) {
                            Icon(Icons.Default.Settings, "Jarvis settings")
                        }
                        if (state.live) IconButton(onClick = { onAction(AssistantAction.Minimize) }, modifier = Modifier.size(48.dp)) {
                            Icon(Icons.Default.KeyboardArrowDown, "Keep call in background", tint = scheme.onSurface)
                        }
                        FilledTonalIconButton(onClick = { onAction(AssistantAction.Camera) },
                            enabled = state.live && state.call?.phase == VoiceCallPhase.ACTIVE && !cameraBusy && consent == null,
                            modifier = Modifier.size(48.dp)) {
                            Icon(Icons.Default.PhotoCamera, "Take photo for Jarvis")
                        }
                    }
                }
            }
        }
    }
}
