package sh.zeron.android.ui

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.toShape
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.material3.pulltorefresh.PullToRefreshDefaults
import androidx.compose.material3.pulltorefresh.rememberPullToRefreshState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import sh.zeron.android.core.AppMode
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.PhoneEngine
import sh.zeron.android.design.GeistMono
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import sh.zeron.runtime.RuntimePermissions
import sh.zeron.runtime.RuntimeState

/**
 * Start the engine the way the platform wants it: the notification permission
 * (the engine's status and finished sessions) and then the battery exemption
 * are asked for only now (docs/android.md § Android platform constraints).
 */
@Composable
fun rememberEngineStarter(model: AppModel): () -> Unit {
    val activity = LocalContext.current as Activity
    val notifications = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        RuntimePermissions.requestIgnoreBatteryOptimizations(activity)
    }
    return remember(model, activity) {
        {
            model.startEngine()
            if (Build.VERSION.SDK_INT >= 33 && RuntimePermissions.needsNotificationPermission(activity)) {
                notifications.launch(Manifest.permission.POST_NOTIFICATIONS)
            } else {
                RuntimePermissions.requestIgnoreBatteryOptimizations(activity)
            }
        }
    }
}

/** Phone mode before the engine answers: set up / start / progress / failure. */
@Composable
fun PhoneSetupScreen(model: AppModel) {
    val phone = model.phone
    val state by phone.state.collectAsState()
    val start = rememberEngineStarter(model)
    var confirmReset by remember { mutableStateOf(false) }
    Box(Modifier.fillMaxSize().safeDrawingPadding(), contentAlignment = Alignment.TopCenter) {
        Column(
            Modifier.widthIn(max = 560.dp).fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Spacer(Modifier.height(48.dp))
            Box(
                Modifier.size(112.dp).clip(MaterialShapes.Cookie9Sided.toShape()).background(MaterialTheme.colorScheme.primaryContainer),
                contentAlignment = Alignment.Center,
            ) { ZIcon(ZIcons.Phone, null, Modifier.size(52.dp), tint = MaterialTheme.colorScheme.onPrimaryContainer) }
            Spacer(Modifier.height(24.dp))
            Text("Agents on this phone", style = MaterialTheme.typography.headlineMediumEmphasized, textAlign = TextAlign.Center)
            Spacer(Modifier.height(8.dp))
            Text(
                "Zeron runs its engine and real coding agents in a small Linux system on this device.",
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(28.dp))
            EngineCard(phone, state)
            Spacer(Modifier.height(20.dp))
            val wide = Modifier.fillMaxWidth().heightIn(min = ButtonDefaults.MediumContainerHeight)
            when (state) {
                RuntimeState.NotInstalled, RuntimeState.Stopped, is RuntimeState.Failed -> Button(
                    onClick = start,
                    enabled = phone.isSupportedAbi,
                    modifier = wide,
                    shapes = ButtonDefaults.shapes(),
                    contentPadding = ButtonDefaults.contentPaddingFor(ButtonDefaults.MediumContainerHeight),
                ) {
                    Text(
                        when (state) {
                            RuntimeState.NotInstalled -> "Set up"
                            is RuntimeState.Failed -> "Try again"
                            else -> "Start engine"
                        },
                        style = ButtonDefaults.textStyleFor(ButtonDefaults.MediumContainerHeight),
                    )
                }
                is RuntimeState.Running -> Row(verticalAlignment = Alignment.CenterVertically) {
                    LoadingIndicator(Modifier.size(32.dp))
                    Spacer(Modifier.width(8.dp))
                    Text("Connecting to the engine…", style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                else -> FilledTonalButton(onClick = { model.stopEngine() }, modifier = wide, shapes = ButtonDefaults.shapes()) {
                    Text("Stop", style = ButtonDefaults.textStyleFor(ButtonDefaults.MediumContainerHeight))
                }
            }
            if (state is RuntimeState.Failed) {
                Spacer(Modifier.height(12.dp))
                OutlinedButton(onClick = { confirmReset = true }, modifier = wide, shapes = ButtonDefaults.shapes()) {
                    Text("Reset engine…", color = MaterialTheme.colorScheme.error, style = ButtonDefaults.textStyleFor(ButtonDefaults.MediumContainerHeight))
                }
                Spacer(Modifier.height(20.dp))
                ChildProcessHint(Modifier.fillMaxWidth())
                Spacer(Modifier.height(12.dp))
                LogBox(PhoneEngine.stripAnsi((state as RuntimeState.Failed).logTail).ifBlank { phone.logTail(60) }, Modifier.fillMaxWidth())
            }
            Spacer(Modifier.height(24.dp))
            TextButton(onClick = { model.signOut() }) { Text("Use Zeron another way") }
            Spacer(Modifier.height(24.dp))
        }
    }
    if (confirmReset) ResetDialog(onDismiss = { confirmReset = false }) { model.resetEngine() }
}

/** Settings → On-device engine: state, start/stop/reset, keeping it alive, the log. */
@Composable
fun EngineScreen(model: AppModel, onBack: () -> Unit) {
    val phone = model.phone
    val state by phone.state.collectAsState()
    val start = rememberEngineStarter(model)
    val activity = LocalContext.current as Activity
    val clipboard = LocalClipboardManager.current
    var confirmReset by remember { mutableStateOf(false) }
    // Permission rows re-read when the user comes back from a system screen.
    var resumes by remember { mutableIntStateOf(0) }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { resumes++ }
    val battery = remember(resumes) { RuntimePermissions.isIgnoringBatteryOptimizations(activity) }
    val notify = remember(resumes) { model.notifier.permitted }
    val notifications = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { resumes++ }
    var log by remember { mutableStateOf("") }
    LaunchedEffect(Unit) {
        while (true) {
            log = withContext(Dispatchers.IO) { runCatching { phone.logTail(160) }.getOrDefault("") }
            delay(2000)
        }
    }

    SubPage(title = "On-device engine", subtitle = "Linux guest · local edge", onBack = onBack) {
        item { EngineCard(phone, state, Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) }
        item {
            Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                when (state) {
                    is RuntimeState.Running, RuntimeState.Starting, is RuntimeState.Bootstrapping ->
                        FilledTonalButton(onClick = { model.stopEngine() }, modifier = Modifier.weight(1f), shapes = ButtonDefaults.shapes()) {
                            ZIcon(ZIcons.Stop, null, Modifier.size(ButtonDefaults.IconSize))
                            Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                            Text("Stop")
                        }
                    else -> Button(onClick = start, enabled = phone.isSupportedAbi, modifier = Modifier.weight(1f), shapes = ButtonDefaults.shapes()) {
                        ZIcon(ZIcons.Restart, null, Modifier.size(ButtonDefaults.IconSize))
                        Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                        Text(if (state == RuntimeState.NotInstalled) "Set up" else "Start")
                    }
                }
                OutlinedButton(onClick = { confirmReset = true }, modifier = Modifier.weight(1f), shapes = ButtonDefaults.shapes()) {
                    ZIcon(ZIcons.Delete, null, Modifier.size(ButtonDefaults.IconSize), tint = MaterialTheme.colorScheme.error)
                    Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                    Text("Reset…", color = MaterialTheme.colorScheme.error)
                }
            }
        }
        sectionTitle("Keep it running")
        item {
            val rows = if (state is RuntimeState.Failed) 3 else 2
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                SegmentedListItem(
                    onClick = { if (!battery) RuntimePermissions.requestIgnoreBatteryOptimizations(activity) },
                    shapes = segmentedShapes(0, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Restart) },
                    supportingContent = { Text(if (battery) "Allowed" else "Android may pause the engine — tap to allow") },
                    trailingContent = { StatusDot(battery) },
                ) { Text("Unrestricted battery") }
                SegmentedListItem(
                    onClick = {
                        if (notify) return@SegmentedListItem
                        if (Build.VERSION.SDK_INT >= 33 && activity.shouldShowRequestPermissionRationale(Manifest.permission.POST_NOTIFICATIONS)) {
                            notifications.launch(Manifest.permission.POST_NOTIFICATIONS)
                        } else {
                            // Never asked, or denied for good: the system screen always works.
                            activity.startActivity(
                                Intent(android.provider.Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                                    .putExtra(android.provider.Settings.EXTRA_APP_PACKAGE, activity.packageName),
                            )
                        }
                    },
                    shapes = segmentedShapes(1, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Bell) },
                    supportingContent = { Text(if (notify) "Engine status and finished sessions" else "Off — tap to allow") },
                    trailingContent = { StatusDot(notify) },
                ) { Text("Notifications") }
                if (state is RuntimeState.Failed) ChildProcessHint(Modifier, shapes = segmentedShapes(2, rows))
            }
        }
        sectionTitle("Log")
        item {
            Column(Modifier.padding(horizontal = 16.dp)) {
                LogBox(log.ifBlank { "No log yet." }, Modifier.fillMaxWidth())
                Spacer(Modifier.height(8.dp))
                TextButton(onClick = { clipboard.setText(AnnotatedString(phone.logTail(1000))) }) {
                    ZIcon(ZIcons.Copy, null, Modifier.size(ButtonDefaults.IconSize))
                    Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                    Text("Copy log")
                }
            }
        }
    }
    if (confirmReset) ResetDialog(onDismiss = { confirmReset = false }) { model.resetEngine() }
}

/** The engine's state on a tonal card: what it is doing, and how far along. */
@Composable
fun EngineCard(phone: PhoneEngine, state: RuntimeState, modifier: Modifier = Modifier) {
    val (container, content) = when (state) {
        is RuntimeState.Running -> MaterialTheme.colorScheme.primaryContainer to MaterialTheme.colorScheme.onPrimaryContainer
        is RuntimeState.Failed -> MaterialTheme.colorScheme.errorContainer to MaterialTheme.colorScheme.onErrorContainer
        else -> MaterialTheme.colorScheme.surfaceContainerHigh to MaterialTheme.colorScheme.onSurface
    }
    val detail = when (state) {
        RuntimeState.NotInstalled -> "Sets up a small Linux system (Alpine) with git, Node and the engine. Nothing leaves this phone."
        is RuntimeState.Bootstrapping -> state.step
        RuntimeState.Starting -> "Starting the engine…"
        is RuntimeState.Running -> "${state.deviceName} · ${state.edgeUrl.removePrefix("http://")}"
        RuntimeState.Stopped -> "Agents on this phone are paused."
        is RuntimeState.Failed -> state.reason
    }
    val busy = state is RuntimeState.Bootstrapping || state == RuntimeState.Starting
    Surface(shape = RoundedCornerShape(32.dp), color = container, contentColor = content, modifier = modifier.fillMaxWidth()) {
        Column(Modifier.padding(20.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) {
                    if (busy) {
                        LoadingIndicator(Modifier.size(48.dp))
                    } else {
                        Box(
                            Modifier.size(48.dp).clip(MaterialShapes.Cookie9Sided.toShape()).background(content.copy(alpha = 0.12f)),
                            contentAlignment = Alignment.Center,
                        ) { ZIcon(if (state is RuntimeState.Failed) ZIcons.Warning else ZIcons.Terminal, null, Modifier.size(24.dp)) }
                    }
                }
                Spacer(Modifier.width(16.dp))
                Column(Modifier.weight(1f)) {
                    Text(PhoneEngine.stateLabel(state), style = MaterialTheme.typography.titleLargeEmphasized)
                    Text(detail, style = MaterialTheme.typography.bodyMedium, color = content.copy(alpha = 0.8f), maxLines = 6)
                }
            }
            if (busy) {
                Spacer(Modifier.height(16.dp))
                val progress = (state as? RuntimeState.Bootstrapping)?.progress
                if (progress != null) {
                    LinearWavyProgressIndicator(progress = { progress }, modifier = Modifier.fillMaxWidth())
                } else {
                    LinearWavyProgressIndicator(Modifier.fillMaxWidth())
                }
            }
            if (!phone.isSupportedAbi) {
                Spacer(Modifier.height(12.dp))
                Text("This build doesn't include the engine for this device's CPU.", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error)
            }
        }
    }
}

/** Android 12+ kills apps' extra processes past a cap; this is the switch that lifts it. */
@Composable
private fun ChildProcessHint(modifier: Modifier, shapes: androidx.compose.material3.ListItemShapes = segmentedShapes(0, 1)) {
    val context = LocalContext.current
    SegmentedListItem(
        onClick = { runCatching { context.startActivity(RuntimePermissions.developerOptionsIntent()) } },
        shapes = shapes,
        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
        leadingContent = { IconTile(ZIcons.Warning, MaterialTheme.colorScheme.errorContainer, MaterialTheme.colorScheme.onErrorContainer) },
        supportingContent = { Text("If Android stopped the engine, turn off “Disable child process restrictions” in Developer options.") },
        trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(20.dp)) },
        modifier = modifier,
    ) { Text("Child process limit") }
}

@Composable
private fun LogBox(text: String, modifier: Modifier) {
    // Follow the tail as lines arrive.
    val scroll = rememberScrollState()
    val end = scroll.maxValue
    LaunchedEffect(text, end) { scroll.scrollTo(end) }
    Surface(shape = RoundedCornerShape(20.dp), color = MaterialTheme.colorScheme.surfaceContainerHighest, modifier = modifier) {
        Box(
            Modifier
                .heightIn(min = 120.dp, max = 360.dp)
                .verticalScroll(scroll)
                .horizontalScroll(rememberScrollState())
                .padding(14.dp),
        ) {
            Text(text, style = MaterialTheme.typography.bodySmall, fontFamily = GeistMono, color = MaterialTheme.colorScheme.onSurfaceVariant, softWrap = false)
        }
    }
}

@Composable
private fun StatusDot(ok: Boolean) {
    Box(Modifier.size(10.dp).clip(androidx.compose.foundation.shape.CircleShape).background(if (ok) successColor() else MaterialTheme.colorScheme.error))
}

@Composable
private fun ResetDialog(onDismiss: () -> Unit, onReset: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { ZIcon(ZIcons.Delete, null) },
        title = { Text("Reset the on-device engine?") },
        text = { Text("Deletes the Linux guest with its projects, agents, sign-ins and sessions on this phone. Nothing else is touched.") },
        confirmButton = {
            TextButton(onClick = {
                onDismiss()
                onReset()
            }) { Text("Reset", color = MaterialTheme.colorScheme.error) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/**
 * A pushed settings page: round back button, the expressive title (with
 * optional header [actions]), a list; pull-to-refresh when [onRefresh] is set.
 */
@Composable
fun SubPage(
    title: String,
    subtitle: String?,
    onBack: () -> Unit,
    overlay: @Composable BoxScope.() -> Unit = {},
    actions: @Composable RowScope.() -> Unit = {},
    refreshing: Boolean = false,
    onRefresh: (() -> Unit)? = null,
    content: LazyListScope.() -> Unit,
) {
    val list = rememberLazyListState()
    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        val column = @Composable {
            LazyColumn(
                Modifier.fillMaxSize(),
                state = list,
                contentPadding = PaddingValues(bottom = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 48.dp),
            ) {
                item {
                    Box(Modifier.statusBarsPadding().padding(start = 12.dp, top = 8.dp)) {
                        TonalCircleButton(ZIcons.Back, "Back", onClick = onBack)
                    }
                }
                item { ScreenHeader(title, subtitle, actions = actions) }
                content()
            }
        }
        if (onRefresh == null) {
            column()
        } else {
            val pull = rememberPullToRefreshState()
            PullToRefreshBox(
                isRefreshing = refreshing,
                onRefresh = onRefresh,
                state = pull,
                modifier = Modifier.fillMaxSize(),
                indicator = {
                    PullToRefreshDefaults.LoadingIndicator(
                        state = pull,
                        isRefreshing = refreshing,
                        modifier = Modifier.align(Alignment.TopCenter).padding(WindowInsets.statusBars.asPaddingValues()),
                    )
                },
            ) { column() }
        }
        StatusBarScrim(scrolled = list.firstVisibleItemIndex > 0 || list.firstVisibleItemScrollOffset > 0)
        overlay()
    }
}

fun LazyListScope.sectionTitle(title: String) {
    item {
        Text(
            title,
            style = MaterialTheme.typography.titleSmallEmphasized,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.padding(start = 28.dp, top = 24.dp, bottom = 8.dp),
        )
    }
}

/** Where agents run, for the Settings switch and the sign-in screen. */
fun AppMode.title(): String = when (this) {
    AppMode.Phone -> "This phone"
    AppMode.Account -> "Your computers"
    AppMode.Demo -> "Demo"
}

fun AppMode.detail(): String = when (this) {
    AppMode.Phone -> "The engine and the agents run on this device"
    AppMode.Account -> "Drive agents on your computers with a Zeron account"
    AppMode.Demo -> "An offline workspace with a simulated computer"
}

fun AppMode.icon(): Int = when (this) {
    AppMode.Phone -> ZIcons.Phone
    AppMode.Account -> ZIcons.Laptop
    AppMode.Demo -> ZIcons.Magic
}
