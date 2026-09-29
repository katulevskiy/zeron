package sh.zeron.android.ui.session

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Bitmap
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.Attachments
import sh.zeron.android.core.Banner
import sh.zeron.android.core.ComposerChip
import sh.zeron.android.core.DeliveryMode
import sh.zeron.android.core.QueueAction
import sh.zeron.android.core.SessionController
import sh.zeron.android.core.StagedImage
import sh.zeron.android.design.Anchor
import sh.zeron.android.design.ButtonStyle
import sh.zeron.android.design.CapsuleButton
import sh.zeron.android.design.CircleIconButton
import sh.zeron.android.design.EmptyState
import sh.zeron.android.design.Fonts
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.GlyphKind
import sh.zeron.android.design.LocalColors
import sh.zeron.android.design.LocalOverlays
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.Overlays
import sh.zeron.android.design.StatusGlyph
import sh.zeron.android.design.TopBar
import sh.zeron.android.design.ZText
import sh.zeron.android.design.ZType
import sh.zeron.android.design.anchor
import sh.zeron.android.design.rememberAnchor
import sh.zeron.android.transcript.FrameRelay
import sh.zeron.android.transcript.TranscriptCanvas
import sh.zeron.android.ui.LocalModel
import sh.zeron.android.ui.LocalNavigator
import sh.zeron.android.ui.openUrl
import sh.zeron.android.ui.sessions.archiveWithUndo
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.TranscriptView
import uniffi.zeron_core.reasoningLabel

/**
 * One session: the Rust-laid-out transcript painted on a Canvas under a
 * floating top bar and a bottom stack (status pill, queue, composer or
 * question panel) that rides the keyboard.
 */
@Composable
fun SessionScreen(chatId: String) {
    val model = LocalModel.current
    val client = model.client.collectAsState().value ?: return
    val controller = remember(chatId, client) { SessionController(model, client, chatId) }
    if (!controller.available) {
        UnavailableSession(controller)
        return
    }
    SessionContent(model, client, controller)
}

@Composable
private fun UnavailableSession(controller: SessionController) {
    val nav = LocalNavigator.current
    DisposableEffect(controller) { onDispose { controller.close() } }
    Column(Modifier.fillMaxSize()) {
        TopBar("Session", onBack = { nav.pop() })
        EmptyState(Glyph.Warning, "Can't open this session", "It may have been deleted on another device.", Modifier.fillMaxWidth().padding(top = 80.dp))
    }
}

@Composable
private fun SessionContent(model: AppModel, client: CoreClient, controller: SessionController) {
    val chatId = controller.chatId
    val nav = LocalNavigator.current
    val overlays = LocalOverlays.current
    val c = LocalColors.current
    val context = LocalContext.current
    val density = LocalDensity.current
    val focusManager = LocalFocusManager.current
    val scope = rememberCoroutineScope()
    val chrome by controller.chrome.collectAsState()
    val ws by model.workspace.collectAsState()
    val row = remember(ws, chatId) { model.row(chatId) }

    val relay = remember(controller) { FrameRelay() }
    val engine = remember(controller) { TranscriptView(Fonts.textSystem, relay) }
    val canvas = remember(controller) { TranscriptCanvas(context, engine).also { relay.target = it.listener } }
    val composer = remember(chatId) { ComposerState(model.prefs.draft(chatId)) }
    val focus = remember { FocusRequester() }
    var loaded by remember { mutableStateOf(false) }
    var jumpVisible by remember { mutableStateOf(false) }
    var editingId by remember { mutableStateOf<String?>(null) }
    var stash by remember { mutableStateOf<Pair<String, List<StagedImage>>?>(null) }
    var editPending by remember { mutableStateOf(false) }

    fun saveDraft() = model.prefs.saveDraft(chatId, stash?.first ?: composer.text)

    DisposableEffect(controller) {
        canvas.host = object : TranscriptCanvas.Host {
            override fun toggle(key: ULong) = engine.toggle(key)
            override fun toggleDetail(row: ULong, detail: ULong, open: Boolean) = engine.toggleDetail(row, detail, open)
            override fun openLink(url: String) = openLinkFromTranscript(context, overlays, url)
            override fun openImage(reference: String, bitmap: Bitmap?) = showImage(overlays, bitmap) { controller.loadImage(reference) }
            override fun showDetail(title: String, text: String) = showDetailSheet(context, overlays, title, text)
            override fun copied(text: String) {
                copy(context, text)
                overlays.toast("Copied")
            }
            override fun longPress(index: UInt, copyText: String, messageText: String?) {
                overlays.menu(null, null, buildList {
                    if (copyText.isNotBlank()) add(MenuEntry.Item("Copy", glyph = Glyph.Copy) { copy(context, copyText); overlays.toast("Copied") })
                    if (!messageText.isNullOrBlank() && messageText != copyText) {
                        add(MenuEntry.Item("Copy Message", glyph = Glyph.Copy) { copy(context, messageText); overlays.toast("Copied") })
                    }
                    add(MenuEntry.Item("Copy Transcript", glyph = Glyph.Stack) { copy(context, engine.frame().plainText()); overlays.toast("Transcript copied") })
                })
            }
            override fun tapEmpty() = focusManager.clearFocus()
            override fun followChanged(following: Boolean, distanceFromBottom: Float) {
                jumpVisible = !following && distanceFromBottom > 140f
            }
            override fun frameShown(rows: Int) {
                loaded = true
            }
            override suspend fun loadImage(reference: String): Bitmap? = controller.loadImage(reference)
        }
        controller.attach(engine)
        onDispose {
            saveDraft()
            // Leaving mid-edit: the host lease lapses on its own (a person
            // reviews the row); nothing here can outlive the session handle.
            canvas.host = null
            controller.close()
            canvas.close()
            // Stop the layout thread, then free the Rust handle.
            engine.shutdown()
            engine.close()
        }
    }
    LifecycleEventEffect(Lifecycle.Event.ON_STOP) { saveDraft() }
    // An empty or unreachable session: stop showing the loader eventually.
    LaunchedEffect(Unit) {
        delay(8000)
        loaded = true
    }
    SideEffect {
        canvas.colors = c
        canvas.widgets.uploadProgress = chrome.uploadProgress
    }

    fun endEdit(commit: String?) {
        editingId = null
        stash?.let { (text, images) ->
            composer.setText(text)
            composer.images.clear()
            composer.images.addAll(images)
        }
        stash = null
        scope.launch { runCatching { controller.finishEdit(commit) } }
    }

    fun beginEdit(id: String) {
        if (editPending) return
        editPending = true
        scope.launch {
            try {
                if (editingId != null) endEdit(null)
                val text = runCatching { controller.beginEdit(id) }.getOrNull()
                if (text == null) {
                    overlays.alert("Can't edit right now", "Another device is editing this message, or it was just sent.")
                    return@launch
                }
                if (stash == null) stash = composer.text to composer.images.toList()
                editingId = id
                composer.images.clear()
                composer.setText(text)
                runCatching { focus.requestFocus() }
            } finally {
                editPending = false
            }
        }
    }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(Attachments.MAX_IMAGES)) { uris ->
        if (uris.isNotEmpty()) scope.launch { composer.addImages(Attachments.stage(context, uris)) }
    }

    var screenHeight by remember { mutableFloatStateOf(0f) }
    var stackTop by remember { mutableFloatStateOf(0f) }
    val topInsetDp = with(density) { WindowInsets.statusBars.getTop(this).toDp() } + 56.dp
    SideEffect {
        canvas.topInset = topInsetDp.value
        if (screenHeight > 0f && stackTop > 0f) canvas.bottomInset = (screenHeight - stackTop) / density.density + 8f
    }

    Box(
        Modifier
            .fillMaxSize()
            .onSizeChanged { screenHeight = it.height.toFloat() },
    ) {
        AndroidView(
            factory = { canvas },
            modifier = Modifier
                .fillMaxSize()
                .graphicsLayer { alpha = if (loaded) 1f else 0f },
        )
        if (!loaded) {
            Box(Modifier.fillMaxSize().padding(bottom = 80.dp), contentAlignment = Alignment.Center) {
                StatusGlyph(GlyphKind.Spinner, size = 21.dp)
            }
        }
        // Soft top edge: the transcript fades under the bar.
        Box(
            Modifier
                .fillMaxWidth()
                .background(Brush.verticalGradient(0f to c.background, 0.8f to c.background.copy(alpha = 0.97f), 1f to c.background.copy(alpha = 0f)))
                .padding(bottom = 18.dp),
        ) {
            val options = rememberAnchor()
            TopBar(chrome.title.ifEmpty { row?.title ?: "Session" }, subtitle = chrome.subtitle, onBack = { nav.pop() }) {
                CircleIconButton(Glyph.Ellipsis, Modifier.anchor(options), label = "Session options") {
                    sessionOptions(model, overlays, nav, options, chatId, context) { engine.frame().plainText() }
                }
            }
        }
        // Soft bottom edge behind the composer stack.
        val fadeHeight = with(density) { (screenHeight - stackTop).coerceAtLeast(0f).toDp() } + 44.dp
        Box(
            Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .height(fadeHeight)
                .background(Brush.verticalGradient(0f to c.background.copy(alpha = 0f), 0.4f to c.background.copy(alpha = 0.95f), 0.55f to c.background)),
        )
        Column(
            Modifier
                .align(Alignment.BottomCenter)
                .windowInsetsPadding(WindowInsets.ime.union(WindowInsets.navigationBars))
                .padding(horizontal = 12.dp)
                .padding(bottom = 8.dp)
                .widthIn(max = 768.dp)
                .fillMaxWidth()
                .onGloballyPositioned { stackTop = it.positionInRoot().y },
        ) {
            val banner = if (editingId != null) Banner.Editing else chrome.banner
            AnimatedVisibility(banner != null, enter = fadeIn() + scaleIn(initialScale = 0.9f), exit = fadeOut() + scaleOut(targetScale = 0.9f)) {
                Row(Modifier.padding(bottom = 8.dp)) {
                    StatusPill(banner ?: Banner.Offline) {
                        when {
                            editingId != null -> endEdit(null)
                            chrome.banner == Banner.NotDelivered -> controller.retryDelivery()
                        }
                    }
                }
            }
            val questions = chrome.questions
            AnimatedVisibility(chrome.queue.isNotEmpty() && questions == null) {
                Box(Modifier.padding(bottom = 8.dp)) {
                    QueuePanel(chrome.queue) { id, action -> if (action == QueueAction.Edit) beginEdit(id) else controller.queueAction(id, action) }
                }
            }
            if (chrome.error != null) {
                Row(Modifier.padding(bottom = 8.dp)) {
                    StatusPill(Banner.Failed(chrome.error!!)) { controller.clearQueueError() }
                }
            }
            if (questions != null) {
                QuestionPanel(questions.first, questions.second) { answers -> controller.answer(questions.first, answers) }
            } else {
                Composer(
                    state = composer,
                    placeholder = if (editingId != null) "Edit queued message" else chrome.placeholder,
                    running = chrome.running && editingId == null,
                    canSteer = chrome.canSteer,
                    chips = chrome.chips,
                    focus = focus,
                    onChip = { chip, anchor -> chipMenu(model, overlays, controller, context, chip, anchor, chatId) },
                    onAttach = { picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) },
                    onSend = { text, images, mode ->
                        if (editingId != null) {
                            endEdit(text)
                            SendResult.Replaced
                        } else {
                            // An immediate send gets the runway (the prompt glides to
                            // the top); one queued behind a live turn just follows.
                            val queued = chrome.running && mode == DeliveryMode.Queue
                            if (controller.send(text, images, mode)) {
                                focusManager.clearFocus()
                                if (queued) canvas.expectQueuedTurn() else canvas.beginOwnTurn()
                                model.prefs.saveDraft(chatId, "")
                                SendResult.Sent
                            } else {
                                SendResult.Kept
                            }
                        }
                    },
                    onStop = { controller.stop() },
                    mentionSearch = { q -> controller.searchFiles(q) },
                )
            }
        }
        // Jump to latest.
        val jumpBottom = with(density) { (screenHeight - stackTop).coerceAtLeast(0f).toDp() } + 12.dp
        AnimatedVisibility(
            jumpVisible,
            modifier = Modifier
                .align(Alignment.BottomEnd)
                .padding(end = 16.dp, bottom = jumpBottom),
            enter = fadeIn() + scaleIn(initialScale = 0.6f),
            exit = fadeOut() + scaleOut(targetScale = 0.6f),
        ) {
            CircleIconButton(Glyph.ArrowDown, label = "Jump to latest") { canvas.scrollToBottom(true) }
        }
    }
}

private fun sessionOptions(model: AppModel, overlays: Overlays, nav: sh.zeron.android.ui.Navigator, anchor: Anchor, chatId: String, context: Context, transcript: () -> String) {
    val row = model.row(chatId)
    val pinned = row?.pinned ?: false
    overlays.menu(anchor, row?.title, buildList {
        add(MenuEntry.Item(if (pinned) "Unpin" else "Pin", glyph = Glyph.Pin) { model.setPinned(chatId, !pinned) })
        add(MenuEntry.Item("Rename…", glyph = Glyph.Pencil) {
            overlays.prompt("Rename Session", initial = row?.takeIf { it.hasTitle }?.title ?: "", placeholder = "Title", action = "Rename") { model.rename(chatId, it) }
        })
        add(MenuEntry.Item("Copy Transcript", glyph = Glyph.Copy) {
            copy(context, transcript())
            overlays.toast("Transcript copied")
        })
        row?.pullRequest?.let { pr ->
            add(MenuEntry.Item("Open Pull Request #${pr.number}", glyph = Glyph.ArrowUpRight) { openUrl(context, pr.url) })
        }
        add(MenuEntry.Divider)
        add(MenuEntry.Item("Archive", glyph = Glyph.Archive) {
            archiveWithUndo(model, overlays, chatId)
            nav.pop()
        })
        add(MenuEntry.Item("Delete…", glyph = Glyph.Trash, destructive = true) {
            overlays.confirm("Delete this session?", "The session and its transcript are removed on every device.", "Delete", destructive = true) {
                model.delete(chatId)
                nav.pop()
            }
        })
    })
}

private fun chipMenu(model: AppModel, overlays: Overlays, controller: SessionController, context: Context, chip: ComposerChip, anchor: Anchor, chatId: String) {
    val row = model.row(chatId) ?: return
    when (chip.kind) {
        ComposerChip.Kind.Model -> overlays.menuAsync(anchor, "Model") {
            controller.models().map { m ->
                MenuEntry.Item(m.label, subtitle = m.description, checked = m.id == row.model) {
                    controller.setConfig { it.copy(model = m.id) }
                }
            }
        }
        ComposerChip.Kind.Effort -> overlays.menuAsync(anchor, "Reasoning effort") {
            val models = controller.models()
            val levels = models.firstOrNull { it.id == row.model }?.reasoningLevels ?: models.firstOrNull()?.reasoningLevels ?: emptyList()
            levels.map { l -> MenuEntry.Item(reasoningLabel(l), checked = l == row.reasoning) { controller.setConfig { it.copy(reasoning = l) } } }
        }
        ComposerChip.Kind.PrOpen, ComposerChip.Kind.PrMerged, ComposerChip.Kind.PrClosed -> {
            val pr = row.pullRequest ?: return
            overlays.menu(anchor, pr.title, listOf(
                MenuEntry.Item("Open Pull Request", glyph = Glyph.ArrowUpRight) { openUrl(context, pr.url) },
                MenuEntry.Item("Copy Link", glyph = Glyph.Link) { copy(context, pr.url); overlays.toast("Link copied") },
            ))
        }
        ComposerChip.Kind.Branch -> overlays.menu(anchor, chip.title, listOf(
            MenuEntry.Item("Copy Branch Name", glyph = Glyph.Copy) { copy(context, chip.title); overlays.toast("Copied") },
        ))
        else -> Unit
    }
}

fun copy(context: Context, text: String) {
    val cm = context.getSystemService(ClipboardManager::class.java)
    cm.setPrimaryClip(ClipData.newPlainText("Zeron", text))
}

private fun openLinkFromTranscript(context: Context, overlays: Overlays, url: String) {
    val lower = url.lowercase()
    if (lower.startsWith("http://") || lower.startsWith("https://") || lower.startsWith("mailto:")) {
        openUrl(context, url)
    } else {
        // File links point into the host's checkout: nothing on this phone opens them.
        copy(context, url.removePrefix("zeron-file:"))
        overlays.toast("Path copied")
    }
}

private fun showImage(overlays: Overlays, initial: Bitmap?, load: suspend () -> Bitmap?) {
    overlays.viewer { dismiss -> ImageViewer(initial, load, dismiss) }
}

private fun showDetailSheet(context: Context, overlays: Overlays, title: String, text: String) {
    overlays.sheet { dismiss ->
        val c = LocalColors.current
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                ZText(title, ZType.sans(17f, FontWeight.SemiBold), c.text, Modifier.weight(1f), maxLines = 2)
                Spacer(Modifier.width(10.dp))
                CapsuleButton("Copy", style = ButtonStyle.Secondary, compact = true, glyph = Glyph.Copy) {
                    copy(context, text)
                    overlays.toast("Copied")
                }
            }
            Spacer(Modifier.height(12.dp))
            Box(
                Modifier
                    .fillMaxWidth()
                    .heightIn(max = 540.dp)
                    .background(c.controlFill, androidx.compose.foundation.shape.RoundedCornerShape(14.dp))
                    .verticalScroll(rememberScrollState())
                    .horizontalScroll(rememberScrollState())
                    .padding(14.dp),
            ) {
                ZText(text, ZType.mono(12.5f), c.text, overflow = androidx.compose.ui.text.style.TextOverflow.Clip)
            }
            Spacer(Modifier.height(12.dp))
            CapsuleButton("Done", Modifier.fillMaxWidth(), style = ButtonStyle.Secondary, compact = true) { dismiss() }
        }
    }
}
