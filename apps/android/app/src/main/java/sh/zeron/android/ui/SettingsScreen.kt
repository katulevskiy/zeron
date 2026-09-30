package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ButtonGroupDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.ToggleButton
import androidx.compose.material3.ToggleButtonDefaults
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.AppMode
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.PhoneEngine
import sh.zeron.android.design.ThemeMode
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.coreVersion
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SettingsScreen(model: AppModel, onOpen: (String) -> Unit) {
    val appearance by model.appearance.collectAsState()
    val mode by model.mode.collectAsState()
    val workspace by model.workspace.collectAsState()
    val devices = workspace?.devices.orEmpty()
    val list = androidx.compose.foundation.lazy.rememberLazyListState()
    var developer by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(model.developer) }
    var versionTaps by androidx.compose.runtime.remember { androidx.compose.runtime.mutableIntStateOf(0) }
    Box(Modifier.fillMaxSize()) {
    LazyColumn(
        Modifier.fillMaxSize(),
        state = list,
        contentPadding = PaddingValues(top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 8.dp, bottom = 140.dp),
    ) {
        item { ScreenHeader("Settings", null) }
        item {
            // Account: a tonal hero card with a shaped monogram.
            Surface(
                shape = RoundedCornerShape(32.dp),
                color = MaterialTheme.colorScheme.primaryContainer,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
            ) {
                Row(Modifier.padding(20.dp), verticalAlignment = Alignment.CenterVertically) {
                    Box(
                        Modifier.size(64.dp).clip(MaterialShapes.Cookie9Sided.toShape()).background(MaterialTheme.colorScheme.primary),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(model.accountName.take(1).uppercase(), style = MaterialTheme.typography.headlineSmallEmphasized, color = MaterialTheme.colorScheme.onPrimary)
                    }
                    Spacer(Modifier.width(16.dp))
                    Column {
                        Text(model.accountName, style = MaterialTheme.typography.titleLargeEmphasized, color = MaterialTheme.colorScheme.onPrimaryContainer)
                        Text(model.accountDetail, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onPrimaryContainer.copy(alpha = 0.8f))
                    }
                }
            }
        }
        section("Where agents run")
        item { ModeSettings(model, mode) }
        section("Agents")
        item {
            // The engine page exists only for this phone's engine; agents for any engine device.
            val phone = mode == AppMode.Phone
            val rows = if (phone) 2 else 1
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                if (phone) {
                    val state by model.phone.state.collectAsState()
                    SegmentedListItem(
                        onClick = { onOpen(Routes.ENGINE) },
                        shapes = segmentedShapes(0, rows),
                        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                        leadingContent = { IconTile(ZIcons.Terminal) },
                        supportingContent = { Text(PhoneEngine.stateLabel(state)) },
                        trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(20.dp)) },
                    ) { Text("On-device engine") }
                }
                SegmentedListItem(
                    onClick = { onOpen(Routes.AGENTS) },
                    shapes = segmentedShapes(rows - 1, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Bot) },
                    supportingContent = { Text("Install agents and sign in to their accounts") },
                    trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(20.dp)) },
                ) { Text("Coding agents") }
            }
        }
        if (mode == AppMode.Phone) {
            section("Files")
            item {
                val transfers by model.transfers.list.collectAsState()
                val live = transfers.count { it.state.live }
                Column(Modifier.padding(horizontal = 16.dp)) {
                    SegmentedListItem(
                        onClick = { onOpen(Routes.TRANSFERS) },
                        shapes = segmentedShapes(0, 1),
                        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                        leadingContent = { IconTile(ZIcons.ArrowDown) },
                        supportingContent = {
                            Text(if (live > 0) "$live in progress" else "Send and receive files with your other devices")
                        },
                        trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(20.dp)) },
                    ) { Text("Transfers") }
                }
            }
        }
        section("Appearance")
        item {
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                Surface(shape = segmentedShapes(0, 2).shape, color = cardColor()) {
                    Column(Modifier.padding(16.dp)) {
                        Text("Theme", style = MaterialTheme.typography.titleMedium)
                        Spacer(Modifier.height(12.dp))
                        Row(horizontalArrangement = Arrangement.spacedBy(ButtonGroupDefaults.ConnectedSpaceBetween)) {
                            val modes = listOf(
                                Triple(ThemeMode.System, "System", ZIcons.Monitor),
                                Triple(ThemeMode.Light, "Light", ZIcons.Sun),
                                Triple(ThemeMode.Dark, "Dark", ZIcons.Moon),
                            )
                            modes.forEachIndexed { index, (mode, label, icon) ->
                                ToggleButton(
                                    checked = appearance.mode == mode,
                                    onCheckedChange = { model.setAppearance(appearance.copy(mode = mode)) },
                                    modifier = Modifier.weight(1f).semantics { role = Role.RadioButton },
                                    shapes = when (index) {
                                        0 -> ButtonGroupDefaults.connectedLeadingButtonShapes()
                                        modes.lastIndex -> ButtonGroupDefaults.connectedTrailingButtonShapes()
                                        else -> ButtonGroupDefaults.connectedMiddleButtonShapes()
                                    },
                                ) {
                                    ZIcon(icon, null, Modifier.size(ToggleButtonDefaults.IconSize))
                                    Spacer(Modifier.size(ToggleButtonDefaults.IconSpacing))
                                    Text(label)
                                }
                            }
                        }
                    }
                }
                SegmentedListItem(
                    onClick = { model.setAppearance(appearance.copy(dynamicColor = !appearance.dynamicColor)) },
                    shapes = segmentedShapes(1, 2),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Magic) },
                    supportingContent = { Text("Tint Zeron with your wallpaper's palette") },
                    trailingContent = { Switch(appearance.dynamicColor, { model.setAppearance(appearance.copy(dynamicColor = it)) }) },
                ) { Text("Wallpaper colors") }
            }
        }
        section("Wallpaper")
        item { WallpaperSettings(model) }
        if (devices.isNotEmpty()) {
            section("Devices")
            item {
                Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                    devices.forEachIndexed { i, device ->
                        SegmentedListItem(
                            onClick = {},
                            shapes = segmentedShapes(i, devices.size),
                            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                            leadingContent = {
                                IconTile(
                                    when {
                                        !device.isExecutionHost -> ZIcons.Phone
                                        device.platform == "linux" -> ZIcons.Server
                                        else -> ZIcons.Laptop
                                    },
                                )
                            },
                            supportingContent = {
                                Text(
                                    listOfNotNull(
                                        if (device.isSelf) "This device" else if (device.online) "Online" else "Offline",
                                        device.version?.let { "v$it" },
                                        if (device.sessionCount > 0u) "${device.sessionCount} sessions" else null,
                                    ).joinToString(" · "),
                                )
                            },
                            trailingContent = {
                                Box(
                                    Modifier.size(10.dp).clip(CircleShape).background(
                                        if (device.online || device.isSelf) successColor() else MaterialTheme.colorScheme.outlineVariant,
                                    ),
                                )
                            },
                        ) { Text(device.name) }
                    }
                }
            }
        }
        if (developer && model.phone.isSupportedAbi) {
            section("Developer")
            item { DeveloperSettings(model) }
        }
        section("About")
        item {
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                SegmentedListItem(
                    // Seven taps reveal the developer section (Android's own convention).
                    onClick = {
                        versionTaps++
                        if (versionTaps >= 7 && !developer) {
                            developer = true
                            model.setDeveloper(true)
                        }
                    },
                    shapes = segmentedShapes(0, 2),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Info) },
                    supportingContent = { Text("Zeron for Android · core ${coreVersion()}") },
                ) { Text("Version") }
                SegmentedListItem(
                    onClick = { model.signOut() },
                    shapes = segmentedShapes(1, 2),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Logout, MaterialTheme.colorScheme.errorContainer, MaterialTheme.colorScheme.onErrorContainer) },
                ) {
                    Text(
                        when (mode) {
                            AppMode.Demo -> "Leave demo"
                            AppMode.Phone -> "Leave phone mode"
                            else -> "Sign out"
                        },
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }
        }
    }
    StatusBarScrim(scrolled = list.firstVisibleItemIndex > 0 || list.firstVisibleItemScrollOffset > 0)
    }
}

private fun LazyListScope.section(title: String) {
    item {
        Text(
            title,
            style = MaterialTheme.typography.titleSmallEmphasized,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.padding(start = 28.dp, top = 24.dp, bottom = 8.dp),
        )
    }
}

/**
 * The mode switch: this phone's engine, an account's computers, or the demo.
 * Account without a stored sign-in lands on the sign-in screen.
 */
@Composable
private fun ModeSettings(model: AppModel, current: AppMode?) {
    val modes = buildList {
        if (model.phone.isSupportedAbi) add(AppMode.Phone)
        add(AppMode.Account)
        add(AppMode.Demo)
    }
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
        modes.forEachIndexed { i, m ->
            val selected = m == current
            SegmentedListItem(
                onClick = { model.chooseMode(m) },
                shapes = segmentedShapes(i, modes.size),
                colors = ListItemDefaults.segmentedColors(containerColor = if (selected) MaterialTheme.colorScheme.secondaryContainer else cardColor()),
                leadingContent = { IconTile(m.icon()) },
                supportingContent = { Text(m.detail()) },
                trailingContent = if (selected) {
                    {
                        Box(
                            Modifier.size(28.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary),
                            contentAlignment = Alignment.Center,
                        ) { ZIcon(ZIcons.Check, "Selected", Modifier.size(16.dp), tint = MaterialTheme.colorScheme.onPrimary) }
                    }
                } else {
                    null
                },
            ) { Text(m.title()) }
        }
    }
}

/** A leading glyph on a tonal rounded tile. */
@Composable
fun IconTile(
    icon: Int,
    container: Color = MaterialTheme.colorScheme.secondaryContainer,
    content: Color = MaterialTheme.colorScheme.onSecondaryContainer,
) {
    Box(Modifier.size(40.dp).clip(RoundedCornerShape(14.dp)).background(container), contentAlignment = Alignment.Center) {
        ZIcon(icon, null, Modifier.size(22.dp), tint = content)
    }
}

/** Wallpaper: shown behind new sessions and the sessions list (iOS parity). */
@Composable
private fun WallpaperSettings(model: AppModel) {
    val store = model.wallpaper
    val state by store.state.collectAsState()
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    val picker = androidx.activity.compose.rememberLauncherForActivityResult(
        androidx.activity.result.contract.ActivityResultContracts.PickVisualMedia(),
    ) { uri -> if (uri != null) scope.launch { store.set(uri, "Photo") } }
    var effects by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(false) }
    val rows = if (state.set) 3 else 1
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
        SegmentedListItem(
            onClick = {
                picker.launch(androidx.activity.result.PickVisualMediaRequest(androidx.activity.result.contract.ActivityResultContracts.PickVisualMedia.ImageOnly))
            },
            shapes = segmentedShapes(0, rows),
            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
            leadingContent = { IconTile(ZIcons.Image) },
            supportingContent = { Text(if (state.set) state.name ?: "Photo" else "Shown behind new sessions and the sessions list") },
        ) { Text(if (state.set) "Change wallpaper" else "Choose wallpaper") }
        if (state.set) {
            Box {
                SegmentedListItem(
                    onClick = { effects = true },
                    shapes = segmentedShapes(1, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Magic) },
                    supportingContent = { Text("${sh.zeron.android.core.WallpaperStore.label(state.effect)} — ${sh.zeron.android.core.WallpaperStore.detail(state.effect)}") },
                ) { Text("Effect") }
                ChoiceMenu(effects, { effects = false }, listOf(MenuSection("Effect", sh.zeron.android.core.WallpaperStore.effects.map { e ->
                    MenuChoice(sh.zeron.android.core.WallpaperStore.label(e), e == state.effect) { store.setEffect(e) }
                })))
            }
            SegmentedListItem(
                onClick = { store.remove() },
                shapes = segmentedShapes(2, rows),
                colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                leadingContent = { IconTile(ZIcons.Delete, MaterialTheme.colorScheme.errorContainer, MaterialTheme.colorScheme.onErrorContainer) },
            ) { Text("Remove wallpaper", color = MaterialTheme.colorScheme.error) }
        }
    }
}

/**
 * Developer → Custom server: run this phone's engine against a shared edge
 * (`zeron local-edge` on a computer) instead of its own, so it meets other
 * engines — e.g. to test file transfer (docs/android.md § Custom server).
 */
@Composable
private fun DeveloperSettings(model: AppModel) {
    var edge by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(model.phone.customEdge) }
    var editing by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(false) }
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
        SegmentedListItem(
            onClick = { editing = true },
            shapes = segmentedShapes(0, 1),
            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
            leadingContent = { IconTile(ZIcons.Server) },
            supportingContent = { Text(edge?.url ?: "Off — the engine hosts its own edge on this phone") },
            trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(20.dp)) },
        ) { Text("Custom server") }
    }
    if (editing) {
        CustomServerDialog(edge, onDismiss = { editing = false }) { next ->
            editing = false
            edge = next
            model.phone.setCustomEdge(next)
        }
    }
}

@Composable
private fun CustomServerDialog(current: sh.zeron.runtime.CustomEdge?, onDismiss: () -> Unit, onSave: (sh.zeron.runtime.CustomEdge?) -> Unit) {
    var url by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(current?.url ?: "http://") }
    var token by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(current?.token ?: "") }
    val problem = sh.zeron.runtime.CustomEdge.problem(url, token)
    androidx.compose.material3.AlertDialog(
        onDismissRequest = onDismiss,
        icon = { ZIcon(ZIcons.Server, null) },
        title = { Text("Custom server") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(
                    "The engine joins this edge (zeron local-edge) instead of hosting its own, and restarts. Use it to meet engines on other machines.",
                    style = MaterialTheme.typography.bodyMedium,
                )
                androidx.compose.material3.OutlinedTextField(
                    url, { url = it },
                    label = { Text("Server URL") },
                    placeholder = { Text("http://10.0.2.2:27720") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                androidx.compose.material3.OutlinedTextField(
                    token, { token = it },
                    label = { Text("Token") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                if (problem != null && token.isNotEmpty()) {
                    Text(problem, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                }
            }
        },
        confirmButton = {
            androidx.compose.material3.TextButton(
                onClick = { onSave(sh.zeron.runtime.CustomEdge.of(url, token)) },
                enabled = problem == null,
            ) { Text("Save") }
        },
        dismissButton = {
            Row {
                if (current != null) androidx.compose.material3.TextButton(onClick = { onSave(null) }) { Text("Turn off") }
                androidx.compose.material3.TextButton(onClick = onDismiss) { Text("Cancel") }
            }
        },
    )
}
