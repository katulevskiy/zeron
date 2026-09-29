package sh.zeron.android.ui.session

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppMode
import sh.zeron.android.core.Attachments
import sh.zeron.android.core.ComposerChip
import sh.zeron.android.core.ModelChoice
import sh.zeron.android.core.NewSessionDraft
import sh.zeron.android.core.userMessage
import sh.zeron.android.design.BrandMark
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.LocalColors
import sh.zeron.android.design.LocalOverlays
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.TopBar
import sh.zeron.android.design.ZText
import sh.zeron.android.design.ZType
import sh.zeron.android.ui.LocalActivity
import sh.zeron.android.ui.LocalModel
import sh.zeron.android.ui.LocalNavigator
import sh.zeron.android.ui.Route
import sh.zeron.runtime.RuntimePermissions
import uniffi.zeron_core.FileMatch
import uniffi.zeron_core.fallbackModels
import uniffi.zeron_core.harnessLabel
import uniffi.zeron_core.modelLabel
import uniffi.zeron_core.reasoningLabel

/** The last catalog each host reported: chips open on real names while a fresh list loads. */
private val modelCache = HashMap<String, List<ModelChoice>>()

/**
 * "New session" → a composer-first canvas. Context is set with chips (project
 * or device, branch/worktree, model, effort) whose menus load lazily; the
 * composer is focused at once so the common case is: tap, type, send.
 */
@Composable
fun NewSessionScreen(projectId: String?) {
    val model = LocalModel.current
    val nav = LocalNavigator.current
    val overlays = LocalOverlays.current
    val c = LocalColors.current
    val context = LocalContext.current
    val activity = LocalActivity.current
    val focusManager = LocalFocusManager.current
    val scope = rememberCoroutineScope()
    val ws by model.workspace.collectAsState()
    val projects = ws?.projects ?: emptyList()
    val hosts = remember(ws) { model.executionDevices() }
    var draft by remember {
        var d = model.lastDraft
        if (projectId != null) d = d.copy(projectId = projectId, hostId = null, branch = null)
        if (d.projectId != null && projects.none { it.id == d.projectId }) d = d.copy(projectId = null)
        if (d.projectId == null && d.hostId == null) d = d.copy(projectId = (projects.firstOrNull { it.deviceOnline } ?: projects.firstOrNull())?.id)
        if (d.projectId == null && d.hostId == null) d = d.copy(hostId = (hosts.firstOrNull { it.online } ?: hosts.firstOrNull())?.id)
        mutableStateOf(d)
    }
    val composer = remember { ComposerState(model.newSessionText).also { it.images.addAll(model.newSessionImages) } }
    val focus = remember { FocusRequester() }
    var created by remember { mutableStateOf(false) }
    val project = projects.firstOrNull { it.id == draft.projectId }
    val deviceId = project?.deviceId ?: draft.hostId ?: ""
    var models by remember { mutableStateOf(modelCache[deviceId] ?: model.catalogModels()) }

    LaunchedEffect(deviceId) {
        if (deviceId.isEmpty()) return@LaunchedEffect
        modelCache[deviceId]?.let { models = it }
        val fresh = model.models(deviceId)
        if (fresh.isNotEmpty()) {
            modelCache[deviceId] = fresh
            models = fresh
            // The live list holds only harnesses installed on this device: a
            // fresh phone engine may have OpenCode but not the Claude Code
            // default, and sending to a missing harness just fails the turn.
            if (fresh.none { it.harness == draft.harness }) {
                val first = fresh.first()
                draft = draft.copy(harness = first.harness, model = first.id, effort = null)
            }
        }
    }
    // A project made from the "New Project…" item comes back selected.
    LaunchedEffect(nav.projectCreated) {
        val id = nav.projectCreated ?: return@LaunchedEffect
        nav.projectCreated = null
        draft = draft.copy(projectId = id, hostId = null, branch = null)
    }
    LaunchedEffect(Unit) {
        delay(300) // after the push lands
        runCatching { focus.requestFocus() }
    }
    DisposableEffect(Unit) {
        onDispose {
            // Closed without sending: keep what was typed and picked for next time.
            if (!created) {
                model.lastDraft = draft
                model.newSessionText = composer.text
                model.newSessionImages = composer.images.toList()
            }
        }
    }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(Attachments.MAX_IMAGES)) { uris ->
        if (uris.isNotEmpty()) scope.launch { composer.addImages(Attachments.stage(context, uris)) }
    }

    val choice = models.firstOrNull { it.harness == draft.harness && it.id == draft.model } ?: models.firstOrNull { it.harness == draft.harness }
    val chips = buildList {
        if (project != null) {
            add(ComposerChip("project", project.name, kind = ComposerChip.Kind.Project, colorIndex = project.colorIndex.toInt()))
            if (project.gitDetected) add(ComposerChip("branch", if (draft.worktree) "New worktree" else draft.branch ?: "Current branch", kind = ComposerChip.Kind.Branch))
        } else {
            add(ComposerChip("project", "No project", kind = ComposerChip.Kind.Project))
            add(ComposerChip("host", hosts.firstOrNull { it.id == draft.hostId }?.name ?: "Choose device", kind = ComposerChip.Kind.Host))
        }
        // Never the harness name in place of a model: an unlisted model still gets its catalog label.
        val title = choice?.label ?: draft.model?.let { modelLabel(draft.harness, it) } ?: fallbackModels(draft.harness).firstOrNull()?.label ?: harnessLabel(draft.harness)
        add(ComposerChip("model", title, harness = draft.harness, kind = ComposerChip.Kind.Model))
        val efforts = choice?.efforts ?: emptyList()
        if (efforts.isNotEmpty()) add(ComposerChip("effort", reasoningLabel(draft.effort ?: efforts[efforts.size / 2]), kind = ComposerChip.Kind.Effort))
    }

    val search: (suspend (String) -> List<FileMatch>)? = project?.let { p -> { q -> model.searchFiles(p.deviceId, null, p.id, q) } }
    Column(
        Modifier
            .fillMaxSize()
            .windowInsetsPadding(WindowInsets.ime.union(WindowInsets.navigationBars)),
    ) {
        TopBar("New Session", onBack = { nav.pop() })
        Box(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .pointerInput(Unit) { detectTapGestures { focusManager.clearFocus() } },
            contentAlignment = Alignment.Center,
        ) {
            Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.padding(horizontal = 24.dp)) {
                BrandMark(draft.harness, 34.dp)
                Spacer(Modifier.height(14.dp))
                ZText("What are we building?", ZType.sans(22f, FontWeight.SemiBold), c.text, align = TextAlign.Center, maxLines = 2)
                if (project != null) {
                    Spacer(Modifier.height(6.dp))
                    ZText("${project.deviceName ?: model.deviceName(project.deviceId)} · ${project.path}", ZType.mono(12f), c.tertiary, maxLines = 1)
                }
            }
        }
        Composer(
            state = composer,
            placeholder = "Describe the task",
            running = false,
            canSteer = false,
            chips = chips,
            focus = focus,
            alwaysCard = true,
            modifier = Modifier
                .align(Alignment.CenterHorizontally)
                .widthIn(max = 768.dp)
                .padding(horizontal = 12.dp)
                .padding(bottom = 8.dp),
            onChip = { chip, anchor ->
                when (chip.id) {
                    "project" -> overlays.menu(anchor, "Project", buildList {
                        for ((device, list) in projects.groupBy { it.deviceId }) {
                            val first = list.first()
                            add(MenuEntry.Header((first.deviceName ?: model.deviceName(device)) + if (first.deviceOnline) "" else " · offline"))
                            for (p in list) add(MenuEntry.Item(p.name, glyph = if (p.gitDetected) Glyph.Branch else Glyph.Folder, checked = p.id == draft.projectId) {
                                draft = draft.copy(projectId = p.id, hostId = null, branch = null)
                            })
                        }
                        add(MenuEntry.Divider)
                        add(MenuEntry.Item("No Project", glyph = Glyph.Tray, checked = draft.projectId == null) {
                            draft = draft.copy(projectId = null, hostId = draft.hostId ?: (hosts.firstOrNull { it.online } ?: hosts.firstOrNull())?.id)
                        })
                        add(MenuEntry.Item("New Project…", glyph = Glyph.FolderPlus) { nav.push(Route.NewProject(forNewSession = true)) })
                    })
                    "host" -> overlays.menu(anchor, "Run on", hosts.map { h ->
                        MenuEntry.Item(h.name, subtitle = if (h.online) "Online" else "Offline", glyph = if (h.platform == "android") Glyph.Phone else Glyph.Desktop, checked = h.id == draft.hostId) {
                            draft = draft.copy(hostId = h.id)
                        }
                    })
                    "branch" -> {
                        val p = project ?: return@Composer
                        overlays.menuAsync(anchor, "Checkout") {
                            val refs = model.refs(p.id)
                            buildList {
                                add(MenuEntry.Item("New worktree", subtitle = "An isolated checkout for this session", glyph = Glyph.Stack, checked = draft.worktree) { draft = draft.copy(worktree = !draft.worktree) })
                                if (refs.isNotEmpty()) add(MenuEntry.Header("Branch"))
                                for (r in refs) add(MenuEntry.Item(r, checked = r == (draft.branch ?: refs.first())) { draft = draft.copy(branch = r) })
                            }
                        }
                    }
                    "model" -> overlays.menu(anchor, "Model", buildList {
                        for ((h, list) in models.groupBy { it.harness }) {
                            add(MenuEntry.Header(list.first().harnessLabel))
                            for (m in list) add(MenuEntry.Item(m.label, subtitle = m.description, icon = { BrandMark(h, 16.dp) }, checked = m.harness == draft.harness && m.id == draft.model) {
                                draft = draft.copy(harness = m.harness, model = m.id, effort = null)
                            })
                        }
                    })
                    "effort" -> overlays.menu(anchor, "Reasoning effort", (choice?.efforts ?: emptyList()).map { e ->
                        MenuEntry.Item(reasoningLabel(e), checked = e == draft.effort) { draft = draft.copy(effort = e) }
                    })
                }
            },
            onAttach = { picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) },
            onSend = { text, images, _ ->
                val result = model.createSession(draft.withDefaults(choice), text, images)
                result.fold(
                    onSuccess = { chatId ->
                        created = true
                        model.newSessionText = ""
                        model.newSessionImages = emptyList()
                        // Local notifications need the permission (Android 13+): ask once
                        // the first session on this phone's engine starts.
                        if (model.mode.value == AppMode.Phone && !model.prefs.askedNotifications) {
                            model.prefs.askedNotifications = true
                            RuntimePermissions.requestNotificationPermission(activity)
                        }
                        focusManager.clearFocus()
                        nav.replaceTop(Route.Session(chatId))
                        SendResult.Sent
                    },
                    onFailure = {
                        overlays.alert("Couldn't start the session", it.userMessage())
                        SendResult.Kept
                    },
                )
            },
            mentionSearch = search,
        )
    }
}

/** A model picked implicitly (the harness's first) is sent explicitly. */
private fun NewSessionDraft.withDefaults(choice: ModelChoice?): NewSessionDraft =
    if (model == null && choice != null) copy(model = choice.id) else this
