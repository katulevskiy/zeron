package sh.zeron.android.ui.settings

import android.content.Intent
import android.provider.Settings
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import sh.zeron.android.core.PhoneEngine
import sh.zeron.android.design.ButtonStyle
import sh.zeron.android.design.CapsuleButton
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.GlyphKind
import sh.zeron.android.design.Group
import sh.zeron.android.design.Hairline
import sh.zeron.android.design.ListRow
import sh.zeron.android.design.LocalColors
import sh.zeron.android.design.LocalOverlays
import sh.zeron.android.design.ProgressTrack
import sh.zeron.android.design.StatusGlyph
import sh.zeron.android.design.TopBar
import sh.zeron.android.design.ZText
import sh.zeron.android.design.ZType
import sh.zeron.android.design.glass
import sh.zeron.android.ui.LocalActivity
import sh.zeron.android.ui.LocalModel
import sh.zeron.android.ui.LocalNavigator
import sh.zeron.android.ui.session.copy
import sh.zeron.runtime.RuntimePermissions
import sh.zeron.runtime.RuntimeState

/** Settings → On-device engine: state, start/stop/reset, permissions, log tail. */
@Composable
fun RuntimeScreen() {
    val model = LocalModel.current
    val nav = LocalNavigator.current
    val overlays = LocalOverlays.current
    val c = LocalColors.current
    val context = LocalContext.current
    val activity = LocalActivity.current
    val scope = rememberCoroutineScope()
    val phone = model.phone
    val state by phone.state.collectAsState()
    var resumes by remember { mutableIntStateOf(0) }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { resumes++ }
    val battery = remember(resumes) { phone.ignoringBatteryOptimizations }
    val notify = remember(resumes) { model.notifier.permitted }
    var log by remember { mutableStateOf("") }
    LaunchedEffect(Unit) {
        while (true) {
            log = runCatching { phone.logTail(160) }.getOrDefault("")
            delay(2000)
        }
    }

    Column(Modifier.fillMaxSize()) {
        TopBar("On-device engine", subtitle = "Linux guest · local edge", onBack = { nav.pop() })
        Column(
            Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(bottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() + 24.dp),
        ) {
            RuntimeCard(phone, state)
            Row(Modifier.padding(horizontal = 16.dp, vertical = 14.dp)) {
                when (state) {
                    is RuntimeState.Running, RuntimeState.Starting, is RuntimeState.Bootstrapping ->
                        CapsuleButton("Stop", Modifier.weight(1f), style = ButtonStyle.Secondary, glyph = Glyph.Power, compact = true) { phone.stop() }
                    else -> CapsuleButton(if (state == RuntimeState.NotInstalled) "Set Up" else "Start", Modifier.weight(1f), glyph = Glyph.Power, compact = true, enabled = phone.isSupportedAbi) {
                        startEngine(model, activity)
                    }
                }
                Spacer(Modifier.width(10.dp))
                CapsuleButton("Reset…", Modifier.weight(1f), style = ButtonStyle.Destructive, glyph = Glyph.Trash, compact = true) {
                    overlays.confirm("Reset the on-device engine?", "Deletes the Linux guest with its projects, agents, sign-ins and sessions on this phone. Nothing else is touched.", "Reset", destructive = true) {
                        scope.launch { phone.reset() }
                    }
                }
            }
            Group("Keep it running", footer = "Agents keep working with the screen off only if Android lets the engine run in the background.") {
                ListRow(
                    "Unrestricted battery",
                    subtitle = if (battery) "Allowed" else "Android may pause the engine — tap to allow",
                    glyph = Glyph.Battery,
                    tint = if (battery) c.success else c.warning,
                ) { if (!battery) RuntimePermissions.requestIgnoreBatteryOptimizations(activity) }
                Hairline()
                ListRow(
                    "Notifications",
                    subtitle = if (notify) "Allowed — engine status and finished sessions" else "Off — the engine's notification is hidden",
                    glyph = Glyph.Bell,
                    tint = if (notify) c.success else c.warning,
                ) {
                    if (!notify) {
                        if (!model.prefs.askedNotifications) {
                            model.prefs.askedNotifications = true
                            RuntimePermissions.requestNotificationPermission(activity)
                        } else {
                            context.startActivity(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName))
                        }
                    }
                }
                if (state is RuntimeState.Failed) {
                    Hairline()
                    ListRow("Child process limit", subtitle = "If Android killed the engine, disable “child process restrictions” in Developer options", glyph = Glyph.Warning, tint = c.danger, chevron = true) {
                        runCatching { context.startActivity(RuntimePermissions.developerOptionsIntent()) }
                    }
                }
            }
            Group("Log") {
                Column(Modifier.padding(14.dp)) {
                    Box(
                        Modifier
                            .fillMaxWidth()
                            .heightIn(min = 120.dp, max = 360.dp)
                            .background(c.controlFill, RoundedCornerShape(12.dp))
                            .verticalScroll(rememberScrollState(Int.MAX_VALUE))
                            .horizontalScroll(rememberScrollState())
                            .padding(10.dp),
                    ) {
                        ZText(log.ifBlank { "No log yet." }, ZType.mono(11f), c.secondary, overflow = TextOverflow.Clip)
                    }
                    Spacer(Modifier.height(10.dp))
                    CapsuleButton("Copy Log", style = ButtonStyle.Secondary, glyph = Glyph.Copy, compact = true) {
                        copy(context, phone.logTail(1000))
                        overlays.toast("Log copied")
                    }
                }
            }
        }
    }
}

/**
 * Start the engine the way the platform wants it: the battery exemption is
 * asked for only now (docs/android.md § Battery), and relaunches restart it.
 */
fun startEngine(model: sh.zeron.android.core.AppModel, activity: android.app.Activity) {
    model.prefs.phoneAutostart = true
    model.phone.start()
    RuntimePermissions.requestIgnoreBatteryOptimizations(activity)
}

@Composable
fun RuntimeCard(phone: PhoneEngine, state: RuntimeState) {
    val c = LocalColors.current
    val (glyph, detail) = when (state) {
        RuntimeState.NotInstalled -> GlyphKind.Dot(c.statusIdle) to "Sets up a small Linux system (Alpine) with git, Node and the engine. Nothing leaves this phone."
        is RuntimeState.Bootstrapping -> GlyphKind.Spinner to state.step
        RuntimeState.Starting -> GlyphKind.Spinner to "Starting the engine…"
        is RuntimeState.Running -> GlyphKind.Dot(c.success) to "${state.deviceName} · ${state.edgeUrl.removePrefix("http://")}"
        RuntimeState.Stopped -> GlyphKind.Dot(c.statusIdle) to "Agents on this phone are paused."
        is RuntimeState.Failed -> GlyphKind.Dot(c.danger) to state.reason
    }
    Column(
        Modifier
            .padding(horizontal = 16.dp, vertical = 8.dp)
            .fillMaxWidth()
            .glass(c, RoundedCornerShape(22.dp), 4.dp)
            .padding(18.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            StatusGlyph(glyph, size = 16.dp)
            Spacer(Modifier.width(10.dp))
            ZText(phone.stateLabel(state), ZType.sans(19f, FontWeight.SemiBold), c.text, maxLines = 1)
        }
        Spacer(Modifier.height(8.dp))
        ZText(detail, ZType.sans(14.5f), c.secondary, maxLines = 6)
        if (state is RuntimeState.Bootstrapping || state == RuntimeState.Starting) {
            Spacer(Modifier.height(14.dp))
            ProgressTrack((state as? RuntimeState.Bootstrapping)?.progress)
        }
        if (!phone.isSupportedAbi) {
            Spacer(Modifier.height(10.dp))
            ZText("This build doesn't include the engine for this device's CPU.", ZType.sans(13.5f), c.danger)
        }
    }
}
