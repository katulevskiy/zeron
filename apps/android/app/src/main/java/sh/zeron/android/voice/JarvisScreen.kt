package sh.zeron.android.voice

import android.Manifest
import android.provider.Settings
import android.os.SystemClock
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import kotlinx.coroutines.delay
import sh.zeron.android.core.AppModel
import uniffi.zeron_core.*

/** Permission results carry the call generation across activity recreation. */
@Composable
fun JarvisPrompts(model: AppModel) {
    val controller = model.jarvis
    val notifications = sh.zeron.android.ui.rememberNotificationAccess(model)
    val state by controller.state.collectAsState()
    LaunchedEffect(state.call?.phase) {
        if (state.call?.phase == VoiceCallPhase.ACTIVE) notifications.askOnce()
    }
    val assistant by controller.assistantPresentation.collectAsState()
    var launched by rememberSaveable { mutableStateOf<Long?>(null) }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        launched?.let { controller.completePermission(it, granted) }
        launched = null
    }
    val requested by controller.permissionRequest.collectAsState()
    LaunchedEffect(requested, launched, assistant) {
        requested?.takeIf { launched == null && !assistant }?.let { id ->
            launched = id
            controller.permissionLaunched(id)
            launcher.launch(Manifest.permission.RECORD_AUDIO)
        }
    }
    val host by controller.consentRequest.collectAsState()
    host?.takeIf { !assistant }?.let { target ->
        AlertDialog(onDismissRequest = controller::dismissConsent,
            title = { Text("Talk with Jarvis") },
            text = { Text("Your phone sends live microphone audio to OpenAI. Codex on ${target.name} runs Jarvis, uses your ChatGPT voice allowance, and saves the conversation in its Zeron chat. Jarvis can use your connected agents and tools. You can mute or hang up at any time.") },
            confirmButton = { TextButton(onClick = controller::acceptConsent) { Text("Continue") } },
            dismissButton = { TextButton(onClick = controller::dismissConsent) { Text("Cancel") } })
    }
}

@Composable
fun JarvisCompact(model: AppModel) {
    val state by model.jarvis.state.collectAsState()
    Surface(color = MaterialTheme.colorScheme.surfaceContainerHigh) {
        Row(Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = { model.pendingRoute.value = "jarvis" }, modifier = Modifier.weight(1f)) {
                Text("Jarvis · " + if (state.muted) "Muted" else if (state.call?.phase == VoiceCallPhase.ACTIVE) "Live" else "Connecting")
            }
            IconButton(onClick = model.jarvis::toggleMute) { Icon(if (state.muted) Icons.Default.MicOff else Icons.Default.Mic, if (state.muted) "Unmute" else "Mute") }
            IconButton(onClick = model.jarvis::stop) { Icon(Icons.Default.CallEnd, "Hang up", tint = MaterialTheme.colorScheme.error) }
        }
    }
}

@Composable
fun JarvisEntry(model: AppModel) {
    val live by model.jarvis.live.collectAsState()
    FilledTonalButton(onClick = model.jarvis::requestStart, modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp)) {
        Icon(Icons.Default.Mic, contentDescription = null, Modifier.size(20.dp))
        Spacer(Modifier.width(8.dp))
        Text(if (live) "Return to Jarvis" else "Talk with Jarvis")
    }
}

@Composable
fun JarvisScreen(model: AppModel, onBack: () -> Unit, preview: Boolean = false) {
    val controller = model.jarvis
    val actual by controller.state.collectAsState()
    val notifications = sh.zeron.android.ui.rememberNotificationAccess(model)
    val state = if (preview) JarvisState(host = "This phone") else actual
    var elapsed by remember { mutableStateOf("0:00") }
    LaunchedEffect(state.activeSince) {
        while (state.activeSince != null) {
            elapsed = voiceElapsed((SystemClock.elapsedRealtime() - state.activeSince) / 1000)
            delay(1000)
        }
    }
    Scaffold(topBar = {
        TopAppBar(title = { Text("Jarvis") }, navigationIcon = {
            IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
        }, actions = {
            IconButton(onClick = { model.pendingRoute.value = "jarvis-settings" }) { Icon(Icons.Default.Settings, "Jarvis settings") }
        })
    }) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).verticalScroll(rememberScrollState()).padding(24.dp),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(20.dp)) {
            Spacer(Modifier.height(16.dp))
            JarvisOrb(if (state.muted) VoiceOrb.MUTED else state.call?.orb ?: VoiceOrb.IDLE,
                state.call?.microphone ?: 0f, state.call?.speaker ?: 0f, Modifier.size(224.dp))
            val status = when {
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
            if (state.host.isNotEmpty()) Text(state.host + if (state.activeSince != null) " · $elapsed" else "", style = MaterialTheme.typography.bodyMedium)
            state.error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyLarge) }
            val caption = state.call?.caption.orEmpty().takeLast(240)
            if (caption.isNotBlank()) Text(caption, style = MaterialTheme.typography.bodyLarge)
            if (state.live) {
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    FilledTonalIconButton(onClick = controller::toggleMute, modifier = Modifier.size(56.dp)) {
                        Icon(if (state.muted) Icons.Default.MicOff else Icons.Default.Mic, if (state.muted) "Unmute microphone" else "Mute microphone")
                    }
                    FilledTonalIconButton(onClick = controller::toggleSpeaker, modifier = Modifier.size(56.dp)) {
                        Icon(if (state.speaker) Icons.Default.VolumeUp else Icons.Default.PhoneInTalk, if (state.speaker) "Use earpiece" else "Use speaker")
                    }
                    FilledIconButton(onClick = controller::stop, modifier = Modifier.size(56.dp),
                        colors = IconButtonDefaults.filledIconButtonColors(containerColor = MaterialTheme.colorScheme.error)) {
                        Icon(Icons.Default.CallEnd, "Hang up")
                    }
                }
                state.call?.chatId?.let { id -> TextButton(onClick = { model.pendingRoute.value = "chat:$id" }) { Text("Open conversation") } }
                Text("The call continues when you leave this screen. Use the notification to mute or hang up.", style = MaterialTheme.typography.bodySmall)
                if (!notifications.granted) TextButton(onClick = notifications.ask) { Text("Enable call notification") }
            } else {
                Button(onClick = controller::requestStart) { Icon(Icons.Default.Mic, null); Spacer(Modifier.width(8.dp)); Text("Start Jarvis") }
                val hosts by controller.hosts.collectAsState()
                val selected by controller.selectedHost.collectAsState()
                if (state.endReason in listOf(VoiceEndReason.SIGN_IN_REQUIRED, VoiceEndReason.HOST_INCOMPATIBLE)
                    && hosts.any { it.id == selected && it.self }) {
                    TextButton(onClick = { model.pendingRoute.value = "agents" }) { Text("Set up Codex on this phone") }
                }
                TextButton(onClick = { model.pendingRoute.value = "jarvis-settings" }) { Text("Choose device and voice") }
            }
        }
    }
}

@Composable
fun JarvisSettings(model: AppModel, onBack: () -> Unit) {
    val controller = model.jarvis
    val hosts by controller.hosts.collectAsState()
    val selected by controller.selectedHost.collectAsState()
    val voices by controller.voices.collectAsState()
    val voice by controller.selectedVoice.collectAsState()
    val state by controller.state.collectAsState()
    LaunchedEffect(Unit) { controller.refreshHosts() }
    Scaffold(topBar = { TopAppBar(title = { Text("Jarvis settings") }, navigationIcon = {
        IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
    }) }) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).verticalScroll(rememberScrollState()).padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            sh.zeron.android.voice.assistant.JarvisAssistantSetup()
            sh.zeron.android.voice.screen.JarvisScreenSettings(controller)
            HorizontalDivider()
            Text("Execution device", style = MaterialTheme.typography.titleLarge)
            Text("Choose where Codex runs Jarvis and its tools. Audio uses this phone's microphone and speaker.")
            if (hosts.isEmpty()) Text("No compatible device is available yet. Install and sign in to Codex under Settings → Coding agents on this phone, or connect a desktop running Zeron with remote voice enabled.")
            hosts.forEach { host ->
                Surface(onClick = { controller.selectHost(host.id) }, enabled = !state.live && host.online,
                    shape = MaterialTheme.shapes.medium, color = MaterialTheme.colorScheme.surfaceContainer) {
                    Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(selected == host.id, onClick = null)
                        Column { Text(host.name + if (host.self) " (this phone)" else ""); Text(if (host.online) "Online" else "Offline", style = MaterialTheme.typography.bodySmall) }
                    }
                }
            }
            if (state.live) Text("End the current call to change execution device.")
            Text("Voice", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(top = 12.dp))
            Text("Changes apply to your next call.", style = MaterialTheme.typography.bodySmall)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                FilterChip(selected = voice == null, onClick = { controller.selectVoice(null) }, label = { Text("Default") })
                voices.forEach { style -> FilterChip(selected = voice == style, onClick = { controller.selectVoice(style) }, label = { Text(style.replaceFirstChar { it.uppercase() }) }) }
            }
            state.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            Button(onClick = controller::requestStart) { Text(if (state.live) "Return to Jarvis" else "Start Jarvis") }
        }
    }
}

/** Same Rust geometry as desktop and iOS; frames pause with lifecycle/reduced motion. */
@Composable
fun JarvisOrb(orb: VoiceOrb, microphone: Float, speaker: Float, modifier: Modifier = Modifier, tint: Color? = null) {
    val renderer = remember { OrbRenderer(OrbPreset.HERO, orb) }
    DisposableEffect(renderer) { onDispose { renderer.destroy() } }
    val owner = LocalLifecycleOwner.current
    val context = LocalContext.current
    val reduced = Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    var frame by remember { mutableStateOf(renderer.nextFrame(false, reduced)) }
    LaunchedEffect(orb, microphone, speaker) { renderer.setOrb(orb); renderer.setAudioLevels(microphone, speaker); frame = renderer.nextFrame(false, reduced) }
    LaunchedEffect(owner, reduced) {
        while (true) {
            if (owner.lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) frame = renderer.nextFrame(true, reduced)
            delay(if (reduced) 300 else 33)
        }
    }
    val dark = MaterialTheme.colorScheme.background.red < 0.5f
    Canvas(modifier.clearAndSetSemantics { contentDescription = "Jarvis ${orb.name.lowercase().replace('_', ' ')}" }) {
        val scale = size.minDimension / frame.size
        fun color(white: Float, alpha: Float): Color {
            if (tint != null) return tint.copy(alpha = (alpha * 1.8f).coerceIn(0f, 1f))
            val v = if (dark) white else 1f - white
            return Color(v, v, v, alpha)
        }
        val lines = frame.lines
        var i = 0
        while (i + 6 < lines.size) {
            drawLine(color(lines[i+5], lines[i+6]), Offset(lines[i]*scale, lines[i+1]*scale), Offset(lines[i+2]*scale, lines[i+3]*scale), lines[i+4]*scale, StrokeCap.Round)
            i += 7
        }
        val dots = frame.dots
        i = 0
        while (i + 4 < dots.size) {
            drawCircle(color(dots[i+3], dots[i+4]), dots[i+2]*scale, Offset(dots[i]*scale, dots[i+1]*scale))
            i += 5
        }
    }
}
