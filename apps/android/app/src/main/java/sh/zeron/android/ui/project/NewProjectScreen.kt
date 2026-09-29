package sh.zeron.android.ui.project

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppMode
import sh.zeron.android.core.Formatters
import sh.zeron.android.core.userMessage
import sh.zeron.android.design.ButtonStyle
import sh.zeron.android.design.CapsuleButton
import sh.zeron.android.design.Chip
import sh.zeron.android.design.DialogCard
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.GlyphIcon
import sh.zeron.android.design.GlyphKind
import sh.zeron.android.design.LocalColors
import sh.zeron.android.design.LocalOverlays
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.StatusGlyph
import sh.zeron.android.design.TopBar
import sh.zeron.android.design.ZText
import sh.zeron.android.design.ZTextField
import sh.zeron.android.design.ZType
import sh.zeron.android.design.anchor
import sh.zeron.android.design.pressable
import sh.zeron.android.design.rememberAnchor
import sh.zeron.android.ui.LocalModel
import sh.zeron.android.ui.LocalNavigator
import sh.zeron.android.ui.Route
import uniffi.zeron_core.DeviceView
import uniffi.zeron_core.FolderListing

/**
 * Create a project (a device + folder pair): pick a device, browse its
 * folders over the relay (git repos badged) and use one — or clone a
 * repository. On this phone's engine browsing starts in /home/zeron/projects.
 */
@Composable
fun NewProjectScreen(forNewSession: Boolean) {
    val model = LocalModel.current
    val nav = LocalNavigator.current
    val overlays = LocalOverlays.current
    val c = LocalColors.current
    val scope = rememberCoroutineScope()
    val phoneMode = model.mode.value == AppMode.Phone
    val devices = remember { model.executionDevices() }
    var device by remember { mutableStateOf(devices.firstOrNull { it.online } ?: devices.firstOrNull()) }
    var listing by remember { mutableStateOf<FolderListing?>(null) }
    var listingDevice by remember { mutableStateOf<String?>(null) }
    var status by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(false) }
    var creating by remember { mutableStateOf(false) }
    var request by remember { mutableStateOf<String?>(null) }
    var generation by remember { mutableIntStateOf(0) }
    val deviceAnchor = rememberAnchor()

    fun finish(projectId: String) {
        if (forNewSession) {
            nav.projectCreated = projectId
            nav.pop()
        } else {
            nav.replaceTop(Route.Project(projectId))
        }
    }

    LaunchedEffect(device, generation) {
        val d = device ?: return@LaunchedEffect
        loading = true
        status = "Loading…"
        var path = request
        if (path == null && phoneMode) {
            // The phone engine's projects root; make sure it exists first.
            runCatching { model.phone.exec("mkdir -p ${Formatters.shellQuote(model.phone.projectsRoot)}", timeoutMs = 20_000) }
            path = model.phone.projectsRoot
        }
        val result = model.listFolders(d.id, path)
        loading = false
        if (result == null) {
            status = if (d.online) "Couldn't read that folder." else "${d.name} is offline."
            return@LaunchedEffect
        }
        listing = result
        listingDevice = d.id
        status = result.path
    }

    fun open(path: String?) {
        request = path
        generation++
    }

    fun create() {
        val d = device ?: return
        val l = listing ?: return
        if (listingDevice != d.id) return
        creating = true
        scope.launch {
            val git = l.entries.any { it.name == ".git" }
            model.createProject(d.id, l.path, git)
                .onSuccess { finish(it) }
                .onFailure { status = "Couldn't create the project: ${it.userMessage()}" }
            creating = false
        }
    }

    fun clone() {
        val d = device ?: return
        overlays.dialog { dismiss ->
            var url by remember { mutableStateOf("") }
            var busy by remember { mutableStateOf(false) }
            var error by remember { mutableStateOf<String?>(null) }
            val focus = remember { androidx.compose.ui.focus.FocusRequester() }
            fun submit() {
                if (busy || Formatters.repoName(url) == null) return
                busy = true
                scope.launch {
                    model.cloneRepo(d.id, url)
                        .onSuccess {
                            dismiss()
                            finish(it)
                        }
                        .onFailure { error = it.userMessage() }
                    busy = false
                }
            }
            LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
            DialogCard("Clone a repository", if (phoneMode) "Cloned into ${model.phone.projectsRoot} on this phone." else "Cloned by ${d.name}'s engine.") {
                ZTextField(url, { url = it; error = null }, "https://github.com/org/repo.git", mono = true, focus = focus, keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go, onSubmit = { submit() })
                if (error != null) {
                    Spacer(Modifier.height(8.dp))
                    ZText(error!!, ZType.sans(13.5f), c.danger)
                }
                Spacer(Modifier.height(16.dp))
                Row {
                    CapsuleButton("Cancel", Modifier.weight(1f), style = ButtonStyle.Secondary, compact = true, enabled = !busy) { dismiss() }
                    Spacer(Modifier.width(10.dp))
                    CapsuleButton("Clone", Modifier.weight(1f), compact = true, busy = busy, enabled = Formatters.repoName(url) != null) { submit() }
                }
            }
        }
    }

    Box(Modifier.fillMaxSize().imePadding()) {
        Column(Modifier.fillMaxSize()) {
            TopBar(listing?.path?.let { Formatters.lastComponent(it) } ?: "New Project", subtitle = "New project", onBack = { nav.pop() })
            Row(Modifier.padding(horizontal = 16.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                Chip(
                    device?.name ?: "No devices",
                    Modifier.anchor(deviceAnchor),
                    leading = { GlyphIcon(if (phoneMode) Glyph.Phone else Glyph.Desktop, c.secondary, size = 15.dp) },
                ) {
                    overlays.menu(deviceAnchor, "Device", devices.map { d: DeviceView ->
                        MenuEntry.Item(d.name, subtitle = if (d.online) "Online" else "Offline", glyph = Glyph.Desktop, checked = d.id == device?.id) {
                            device = d
                            open(null)
                        }
                    })
                }
                Spacer(Modifier.weight(1f))
                if (loading) StatusGlyph(GlyphKind.Spinner)
            }
            LazyColumn(Modifier.weight(1f), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 6.dp, bottom = 140.dp)) {
                if (device != null) {
                    item("clone") {
                        FolderRow("Clone a git repository…", Glyph.Download, accent = true, subtitle = if (phoneMode) "Into ${model.phone.projectsRoot}" else null) { clone() }
                        Spacer(Modifier.height(10.dp))
                    }
                }
                val l = listing
                if (l != null) {
                    Formatters.parentPath(l.path)?.let { up ->
                        item("..") { FolderRow("Parent folder", Glyph.ArrowUp) { if (!loading) open(up) } }
                    }
                    items(l.entries.filter { it.isDir }, key = { it.name }) { e ->
                        FolderRow(e.name, if (e.isRepo) Glyph.Branch else Glyph.Folder, accent = e.isRepo, chevron = true) {
                            if (!loading) open(Formatters.joinPath(l.path, e.name))
                        }
                    }
                    if (l.truncated) item("truncated") { ZText("Showing the first entries only.", ZType.sans(13f), c.tertiary, Modifier.padding(12.dp)) }
                }
            }
        }
        Column(
            Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .background(Brush.verticalGradient(listOf(c.background.copy(alpha = 0f), c.background, c.background)))
                .windowInsetsPadding(WindowInsets.navigationBars)
                .padding(horizontal = 20.dp, vertical = 12.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            if (status != null) ZText(status!!, ZType.mono(12.5f), c.secondary, maxLines = 2, modifier = Modifier.padding(bottom = 10.dp))
            val name = listing?.path?.let { Formatters.lastComponent(it) }
            CapsuleButton(
                if (name != null) "Use “$name”" else "Choose a folder",
                Modifier.fillMaxWidth(),
                enabled = listing != null && listingDevice == device?.id && !loading,
                busy = creating,
            ) { create() }
        }
    }
}

@Composable
private fun FolderRow(title: String, glyph: Glyph, accent: Boolean = false, chevron: Boolean = false, subtitle: String? = null, onClick: () -> Unit) {
    val c = LocalColors.current
    Row(
        Modifier
            .fillMaxWidth()
            .padding(vertical = 2.dp)
            .background(c.elevated, RoundedCornerShape(14.dp))
            .pressable(highlight = c.controlFill, dim = 1f, shape = RoundedCornerShape(14.dp), onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 13.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        GlyphIcon(glyph, if (accent) c.accent else c.secondary, size = 19.dp)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            ZText(title, ZType.sans(15.5f, FontWeight.Medium), if (accent && !chevron) c.accent else c.text, maxLines = 1)
            if (subtitle != null) ZText(subtitle, ZType.mono(12f), c.tertiary, maxLines = 1)
        }
        if (chevron) GlyphIcon(Glyph.ChevronRight, c.tertiary, size = 13.dp)
    }
}
