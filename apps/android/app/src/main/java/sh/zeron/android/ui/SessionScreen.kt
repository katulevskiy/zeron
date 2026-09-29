@file:OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)

package sh.zeron.android.ui

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import androidx.compose.foundation.background
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.TextButton
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import kotlinx.coroutines.launch
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.AnchoredMenu
import sh.zeron.android.design.ArrowUpMark
import sh.zeron.android.design.AssetIcon
import sh.zeron.android.design.GaugeGlyph
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.PrGlyph
import sh.zeron.android.design.BackChevron
import sh.zeron.android.design.BrandMark
import sh.zeron.android.design.EllipsisMark
import sh.zeron.android.design.LocalZeronColors
import sh.zeron.android.design.PlusMark
import sh.zeron.android.design.ProjectTile
import sh.zeron.android.design.StopMark
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType
import sh.zeron.android.design.glassSurface
import uniffi.zeron_core.BusyPolicy
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.LayoutListener
import uniffi.zeron_core.NewSession
import uniffi.zeron_core.OutgoingAttachment
import uniffi.zeron_core.QueueEditAction
import uniffi.zeron_core.QueueEditFinish
import uniffi.zeron_core.QueueEditStart
import uniffi.zeron_core.SendRequest
import uniffi.zeron_core.SendState
import uniffi.zeron_core.SessionTarget
import uniffi.zeron_core.TranscriptView
import uniffi.zeron_core.UserInputAnswer
import uniffi.zeron_core.UserInputQuestion
import uniffi.zeron_core.fileMentionLink
import uniffi.zeron_core.harnessLabel
import uniffi.zeron_core.modelLabel
import uniffi.zeron_core.reasoningLabel

private class FrameRelay : LayoutListener {
    var onReady: () -> Unit = {}
    override fun frameReady(revision: ULong) {
        onReady()
    }
}

private class Staged(val name: String, val bytes: ByteArray, val preview: Bitmap?)

/** A composer context chip (iOS `ComposerChip`): model, effort, PR/branch, context. */
private data class Chip(val id: String, val title: String, val harness: String? = null, val prState: uniffi.zeron_core.PullRequestState? = null)

private enum class Delivery { Send, Queue, Steer, Interrupt }

@Composable
fun SessionScreen(model: ZeronModel, chatId: String) {
    val colors = LocalZeronColors.current
    val client = model.client ?: return
    val text = model.text ?: return
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val handle = remember(chatId) { client.openSession(chatId) }
    var chrome by remember(chatId) { mutableStateOf(handle.composer()) }
    val row = client.sessionRow(chatId)
    LaunchedEffect(model.epoch) { chrome = handle.composer() }
    val relay = remember { FrameRelay() }
    val engine = remember(chatId) {
        TranscriptView(text, relay).also {
            it.attach(client, chatId)
            handle.setViewAttached(true)
        }
    }
    val images = remember { HashMap<String, Bitmap>() }
    var view by remember { mutableStateOf<TranscriptListView?>(null) }
    var distance by remember { mutableStateOf(0f) }
    var draft by remember(chatId) { mutableStateOf("") }
    var focused by remember { mutableStateOf(false) }
    var staged by remember { mutableStateOf(listOf<Staged>()) }
    var menu by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    var renameText by remember { mutableStateOf("") }
    var editingQueue by remember { mutableStateOf<String?>(null) }
    var lightbox by remember { mutableStateOf<Bitmap?>(null) }
    var detail by remember { mutableStateOf<Pair<String, String>?>(null) }
    var composerHeight by remember { mutableIntStateOf(0) }
    var chipMenu by remember { mutableStateOf<Pair<Chip, Rect>?>(null) }
    var headerPx by remember { mutableIntStateOf(0) }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() } ?: return@rememberLauncherForActivityResult
        val name = uri.lastPathSegment?.substringAfterLast('/') ?: "image.jpg"
        val preview = BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
        staged = staged + Staged(name, bytes, preview)
    }
    DisposableEffect(chatId) {
        onDispose {
            runCatching { handle.setViewAttached(false) }
            runCatching { engine.closeEngine() }
            runCatching { engine.close() }
            runCatching { client.closeSession(chatId) }
            runCatching { handle.close() }
        }
    }
    val running = chrome.live.turnRunning
    val questions = chrome.openInput
    Box(Modifier.fillMaxSize().background(colors.background)) {
        AndroidView(
            modifier = Modifier.fillMaxSize(),
            factory = { ctx ->
                TranscriptListView(ctx).also { created ->
                    created.engine = engine
                    created.faces = model.faces
                    created.colors = colors
                    view = created
                    relay.onReady = { created.post { created.onFrame() } }
                    created.post { created.onFrame() }
                }
            },
            update = { host ->
                view = host
                host.colors = colors
                host.faces = model.faces
                host.onToggle = { engine.toggle(it) }
                host.onToggleDetail = { rowKey, detail, open -> engine.toggleDetail(rowKey, detail, open) }
                host.onCopy = { textToCopy ->
                    val cm = context.getSystemService(android.content.ClipboardManager::class.java)
                    cm.setPrimaryClip(android.content.ClipData.newPlainText("zeron", textToCopy))
                    model.showToast("Copied")
                }
                host.onLink = { url ->
                    if (url.startsWith("http")) {
                        runCatching {
                            context.startActivity(android.content.Intent(android.content.Intent.ACTION_VIEW, android.net.Uri.parse(url)))
                        }
                    } else model.showToast(url)
                }
                host.onImage = { bmp -> lightbox = bmp }
                host.onDetail = { title, body -> detail = title to body }
                host.bottomInsetPx = composerHeight
                host.topInsetPx = headerPx
                host.imageFor = { images[it] }
                host.requestImage = req@{ ref ->
                    if (images.containsKey(ref) || ref.startsWith("pending:")) return@req
                    scope.launch {
                        val bytes = runCatching { client.readAttachment(chrome.host.deviceId, ref) }.getOrNull() ?: return@launch
                        val bmp = BitmapFactory.decodeByteArray(bytes, 0, bytes.size) ?: return@launch
                        images[ref] = bmp
                        host.postInvalidate()
                    }
                }
                host.onDistanceFromBottom = { distance = it }
                relay.onReady = { host.post { host.onFrame() } }
            },
        )
        val density = LocalDensity.current
        val fadeHeight = with(density) { (headerPx + 28).coerceAtLeast(1).toDp() }
        Box(
            Modifier
                .align(Alignment.TopCenter)
                .fillMaxWidth()
                .height(fadeHeight)
                .background(
                    Brush.verticalGradient(
                        0f to colors.background,
                        0.62f to colors.background,
                        1f to colors.background.copy(alpha = 0f),
                    ),
                ),
        )
        Column(
            Modifier
                .align(Alignment.TopCenter)
                .onSizeChanged {
                    headerPx = it.height
                    view?.topInsetPx = it.height
                }
                .statusBarsPadding()
                .padding(horizontal = 12.dp, vertical = 4.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth().height(52.dp)) {
                Box(
                    Modifier.size(44.dp).glassSurface(colors, 22.dp).clickable { model.back() },
                    contentAlignment = Alignment.Center,
                ) { BackChevron(colors.text, Modifier.size(18.dp)) }
                Row(Modifier.weight(1f), horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
                    BrandMark(row?.harness, colors, 18.dp)
                    Spacer(Modifier.width(8.dp))
                    Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.widthIn(max = 230.dp)) {
                        Text(
                            chrome.title.ifBlank { row?.title ?: "Session" },
                            color = colors.text,
                            fontFamily = ZeronType.Sans,
                            fontWeight = FontWeight.SemiBold,
                            fontSize = 16.sp,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        val project = row?.project?.name ?: "No project"
                        val hostName = chrome.host.name
                        Text(
                            if (hostName != null) "$project @ $hostName" else project,
                            color = colors.secondary,
                            fontFamily = ZeronType.Sans,
                            fontSize = 12.sp,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
                Box {
                    Box(
                        Modifier.size(44.dp).glassSurface(colors, 22.dp).clickable { menu = true },
                        contentAlignment = Alignment.Center,
                    ) { EllipsisMark(colors.text, Modifier.size(18.dp)) }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        DropdownMenuItem(text = { Text(if (row?.pinned == true) "Unpin" else "Pin") }, onClick = { menu = false; model.pin(chatId, row?.pinned != true) })
                        DropdownMenuItem(text = { Text("Rename") }, onClick = { menu = false; renameText = chrome.title; renaming = true })
                        DropdownMenuItem(text = { Text("Archive") }, onClick = { menu = false; model.archive(chatId); model.back() })
                    }
                }
            }
        }
        Column(
            Modifier.align(Alignment.BottomCenter).widthIn(max = 768.dp).fillMaxWidth().padding(horizontal = 16.dp).windowInsetsPadding(WindowInsets.ime.union(WindowInsets.navigationBars)).padding(bottom = 6.dp).onSizeChanged { composerHeight = it.height },
        ) {
            if (distance > 140f) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                    Box(
                        Modifier.size(40.dp).glassSurface(colors, 20.dp).clickable { view?.jumpToBottom() },
                        contentAlignment = Alignment.Center,
                    ) { Text("↓", color = colors.text, fontSize = 16.sp) }
                }
                Spacer(Modifier.height(8.dp))
            }
            StatusLine(chrome, colors) {
                if (chrome.sendState == SendState.FAILED) runCatching { handle.retryDelivery() }
            }
            if (chrome.queue.isNotEmpty()) {
                QueueCard(chrome.queue, colors, onNow = { id -> scope.launch { runCatching { handle.sendQueuedNow(id) } } }, onRemove = { id -> scope.launch { runCatching { handle.removeQueued(id) } } }, onEdit = { item ->
                    editingQueue = item.id
                    draft = item.visibleText
                })
                Spacer(Modifier.height(8.dp))
            }
            if (questions != null) {
                QuestionCard(questions.questions, colors) { answers ->
                    runCatching { handle.respondInput(questions.requestId, answers) }
                }
            } else {
                ComposerBar(
                    colors = colors,
                    text = draft,
                    onText = { draft = it },
                    placeholder = "Message",
                    running = running,
                    canSteer = chrome.host.capabilities.midTurnSteering == true,
                    focused = focused,
                    onFocus = { focused = it },
                    chips = chipsFor(row, chrome),
                    onChip = { chip, rect -> if (chip.id != "context") chipMenu = chip to rect },
                    images = staged,
                    onRemoveImage = { staged = staged.filterNot { s -> s === it } },
                    onAttach = { picker.launch("image/*") },
                    onSend = send@{ mode ->
                        val body = draft.trim()
                        if (body.isEmpty() && staged.isEmpty() && mode != Delivery.Send) return@send
                        if (mode == Delivery.Send && running && body.isEmpty() && staged.isEmpty()) {
                            runCatching { handle.interrupt() }
                            return@send
                        }
                        try {
                            if (mode == Delivery.Interrupt && running) handle.interrupt()
                            val editing = editingQueue
                            if (editing != null) {
                                scope.launch {
                                    try {
                                        when (val start = handle.beginQueuedEdit(editing, client.deviceId())) {
                                            is QueueEditStart.Acquired -> {
                                                val finish = handle.finishQueuedEdit(start.lease, QueueEditAction.COMMIT, body)
                                                if (finish != QueueEditFinish.FINISHED) model.showToast("Couldn't save the edit")
                                            }
                                            else -> model.showToast("That message isn't editable right now")
                                        }
                                    } catch (t: Throwable) {
                                        model.showToast(t.message ?: "Couldn't save the edit")
                                    }
                                }
                                editingQueue = null
                            } else {
                                handle.send(
                                    SendRequest(
                                        text = body,
                                        attachments = staged.map { OutgoingAttachment(it.name, "image/jpeg", it.bytes) },
                                        worktree = null,
                                        busy = if (mode == Delivery.Steer) BusyPolicy.STEER else BusyPolicy.QUEUE,
                                    ),
                                )
                            }
                            draft = ""
                            staged = emptyList()
                        } catch (t: Throwable) {
                            model.showToast(t.message ?: "Couldn't send")
                        }
                    },
                    mentionSearch = { q ->
                        val device = row?.deviceId ?: chrome.host.deviceId
                        runCatching { client.searchFiles(device, chatId, row?.project?.id, q) }.getOrDefault(emptyList())
                    },
                    onMention = { path, dir ->
                        draft = draft.replace(Regex("@[^\\s]*$"), fileMentionLink(path, dir) + " ")
                    },
                )
            }
        }
        chipMenu?.let { (chip, anchor) ->
            ChipMenu(chip, anchor, row, chrome, client, chatId, colors, model) { chipMenu = null }
        }
        lightbox?.let { bmp ->
            Box(
                Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.92f)).clickable { lightbox = null },
                contentAlignment = Alignment.Center,
            ) {
                Image(bmp.asImageBitmap(), contentDescription = "Attachment", modifier = Modifier.fillMaxWidth().padding(16.dp), contentScale = ContentScale.Fit)
            }
        }
        detail?.let { (title, body) ->
            AlertDialog(
                onDismissRequest = { detail = null },
                title = { Text(title) },
                text = { Text(body, modifier = Modifier.heightIn(max = 360.dp).verticalScroll(rememberScrollState())) },
                confirmButton = { TextButton(onClick = { detail = null }) { Text("Close") } },
            )
        }
        if (renaming) {
            AlertDialog(
                onDismissRequest = { renaming = false },
                title = { Text("Rename") },
                text = {
                    BasicTextField(
                        value = renameText,
                        onValueChange = { renameText = it },
                        textStyle = TextStyle(color = colors.text, fontFamily = ZeronType.Sans, fontSize = 16.sp),
                        modifier = Modifier.fillMaxWidth(),
                    )
                },
                confirmButton = {
                    TextButton(onClick = {
                        val name = renameText.trim()
                        if (name.isNotEmpty()) model.rename(chatId, name)
                        renaming = false
                    }) { Text("Rename") }
                },
                dismissButton = { TextButton(onClick = { renaming = false }) { Text("Cancel") } },
            )
        }
    }
}

/** The chip menus the iOS session composer shows (CoreSessionSource.chipMenu). */
@Composable
private fun ChipMenu(
    chip: Chip,
    anchor: Rect,
    row: uniffi.zeron_core.SessionRow?,
    chrome: uniffi.zeron_core.ComposerState,
    client: uniffi.zeron_core.CoreClient,
    chatId: String,
    colors: ZeronColors,
    model: ZeronModel,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val harness = row?.harness ?: "claude-code"
    var models by remember(chip.id) { mutableStateOf<List<uniffi.zeron_core.ModelInfo>?>(null) }
    if (chip.id == "model" || chip.id == "effort") {
        LaunchedEffect(chip.id, harness) {
            models = runCatching { client.listModels(chrome.host.deviceId, harness) }.getOrNull()?.takeIf { it.isNotEmpty() }
                ?: runCatching { uniffi.zeron_core.fallbackModels(harness) }.getOrDefault(emptyList())
        }
    }
    fun setConfig(change: (uniffi.zeron_core.ChatConfig) -> uniffi.zeron_core.ChatConfig) {
        val current = runCatching { client.sessionConfig(chatId) }.getOrNull()
            ?: uniffi.zeron_core.ChatConfig(harness = harness, model = row?.model, reasoning = row?.reasoning, modelOptions = emptyMap(), sandbox = uniffi.zeron_core.SandboxLevel.WORKSPACE_WRITE)
        try {
            client.setSessionConfig(chatId, change(current))
            model.refreshPull()
        } catch (t: Throwable) {
            model.showToast(t.message ?: "Couldn't change the session")
        }
    }
    val pr = row?.pullRequest
    val (title, entries) = when (chip.id) {
        "model" -> "Model" to models.orEmpty().map { m ->
            MenuEntry(m.label, subtitle = m.description, checked = m.id == row?.model) { setConfig { it.copy(model = m.id) } }
        }
        "effort" -> {
            val all = models.orEmpty()
            val levels = all.firstOrNull { it.id == row?.model }?.reasoningLevels ?: all.firstOrNull()?.reasoningLevels ?: emptyList()
            "Reasoning effort" to levels.map { l ->
                MenuEntry(reasoningLabel(l), checked = l == row?.reasoning) { setConfig { it.copy(reasoning = l) } }
            }
        }
        "pr" -> (pr?.title ?: "") to listOfNotNull(
            pr?.let { p ->
                MenuEntry("Open Pull Request", icon = { c -> AssetIcon("tool-global", 16.dp, c) }) {
                    runCatching { context.startActivity(android.content.Intent(android.content.Intent.ACTION_VIEW, android.net.Uri.parse(p.url))) }
                }
            },
            pr?.let { p ->
                MenuEntry("Copy Link", icon = { c -> AssetIcon("fileicon-files-link", 16.dp, c) }) {
                    val cm = context.getSystemService(android.content.ClipboardManager::class.java)
                    cm.setPrimaryClip(android.content.ClipData.newPlainText("pull request", p.url))
                    model.showToast("Copied")
                }
            },
        )
        "branch" -> (row?.branch ?: "") to listOf(
            MenuEntry("Copy Branch Name", icon = { c -> AssetIcon("tool-git-branch", 16.dp, c) }) {
                val cm = context.getSystemService(android.content.ClipboardManager::class.java)
                cm.setPrimaryClip(android.content.ClipData.newPlainText("branch", row?.branch ?: ""))
                model.showToast("Copied")
            },
        )
        else -> null to emptyList()
    }
    AnchoredMenu(colors, anchor, title, entries, loading = (chip.id == "model" || chip.id == "effort") && models == null, onDismiss = onDismiss)
}

@Composable
private fun StatusLine(chrome: uniffi.zeron_core.ComposerState, colors: ZeronColors, onTap: () -> Unit) {
    val text = when {
        chrome.sendState == SendState.FAILED -> "Not delivered — tap to retry"
        chrome.sendState == SendState.QUEUED -> "${chrome.host.name ?: "Host"} is offline — will send when it's back"
        chrome.queueError != null -> chrome.queueError
        !chrome.room.connected && chrome.room.retryAtMs != null -> "Reconnecting…"
        else -> null
    } ?: return
    Text(
        text,
        color = if (chrome.sendState == SendState.FAILED) colors.danger else colors.secondary,
        fontFamily = ZeronType.Sans,
        fontSize = 13.sp,
        modifier = Modifier.padding(bottom = 6.dp, start = 8.dp).clickable(onClick = onTap),
    )
}

@Composable
private fun QueueCard(
    items: List<uniffi.zeron_core.QueueItem>,
    colors: ZeronColors,
    onNow: (String) -> Unit,
    onRemove: (String) -> Unit,
    onEdit: (uniffi.zeron_core.QueueItem) -> Unit,
) {
    Column(Modifier.fillMaxWidth().glassSurface(colors, 22.dp).padding(12.dp)) {
        Text("Queued", color = colors.secondary, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 12.sp)
        items.forEach { item ->
            Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(item.visibleText, color = colors.text, fontFamily = ZeronType.Sans, fontSize = 14.sp, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                Text("Edit", color = colors.accent, fontSize = 13.sp, modifier = Modifier.clickable { onEdit(item) }.padding(6.dp))
                Text("Now", color = colors.text, fontSize = 13.sp, modifier = Modifier.clickable { onNow(item.id) }.padding(6.dp))
                Text("Remove", color = colors.danger, fontSize = 13.sp, modifier = Modifier.clickable { onRemove(item.id) }.padding(6.dp))
            }
        }
    }
}

@Composable
private fun QuestionCard(questions: List<UserInputQuestion>, colors: ZeronColors, onSubmit: (List<UserInputAnswer>) -> Unit) {
    var page by remember(questions) { mutableStateOf(0) }
    val picks = remember(questions) { mutableMapOf<String, MutableSet<String>>() }
    val q = questions.getOrNull(page) ?: return
    Column(Modifier.fillMaxWidth().glassSurface(colors, 26.dp).padding(16.dp)) {
        Text("${page + 1} of ${questions.size} · ${q.header}", color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 12.5.sp)
        Spacer(Modifier.height(8.dp))
        Text(q.question, color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
        Spacer(Modifier.height(12.dp))
        q.options.forEach { option ->
            val selected = picks[q.id]?.contains(option) == true
            Text(
                option,
                color = colors.text,
                fontFamily = ZeronType.Sans,
                fontSize = 16.sp,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(vertical = 4.dp)
                    .clip(RoundedCornerShape(12.dp))
                    .background(if (selected) colors.accentSoft else colors.chip.copy(alpha = 0.7f))
                    .clickable {
                        val set = picks.getOrPut(q.id) { mutableSetOf() }
                        if (q.multiSelect) {
                            if (!set.add(option)) set.remove(option)
                        } else {
                            set.clear(); set.add(option)
                            if (page < questions.lastIndex) page++ else onSubmit(finish(questions, picks))
                        }
                    }
                    .padding(horizontal = 14.dp, vertical = 12.dp),
            )
        }
        Row(Modifier.fillMaxWidth().padding(top = 8.dp), horizontalArrangement = Arrangement.SpaceBetween) {
            Text("Back", color = colors.secondary, modifier = Modifier.clickable { if (page > 0) page-- }.padding(8.dp))
            Text(if (page == questions.lastIndex) "Send" else "Next", color = colors.text, fontWeight = FontWeight.SemiBold, modifier = Modifier.clickable {
                if (page < questions.lastIndex) page++ else onSubmit(finish(questions, picks))
            }.padding(8.dp))
        }
    }
}

private fun finish(questions: List<UserInputQuestion>, picks: Map<String, Set<String>>) =
    questions.map { UserInputAnswer(it.id, picks[it.id]?.toList() ?: emptyList()) }

private fun chipsFor(row: uniffi.zeron_core.SessionRow?, chrome: uniffi.zeron_core.ComposerState): List<Chip> {
    val chips = mutableListOf<Chip>()
    val model = row?.modelLabel ?: row?.model?.let { modelLabel(row.harness ?: "", it) } ?: row?.harness?.let { harnessLabel(it) }
    if (model != null) chips.add(Chip("model", model, harness = row?.harness ?: "claude-code"))
    row?.reasoning?.takeIf { it.isNotEmpty() }?.let { chips.add(Chip("effort", reasoningLabel(it))) }
    val pr = row?.pullRequest
    if (pr != null) {
        chips.add(Chip("pr", "${pr.number}", prState = pr.state))
    } else {
        row?.branch?.takeIf { it.isNotEmpty() }?.let { chips.add(Chip("branch", it)) }
    }
    val tokens = chrome.contextUsage?.tokens
    val window = chrome.contextUsage?.window
    if (tokens != null && window != null && window > 0uL) {
        val fraction = tokens.toDouble() / window.toDouble()
        if (fraction >= 0.5) chips.add(Chip("context", "${Math.round(fraction * 100)}% context"))
    }
    return chips
}

@Composable
private fun ChipView(chip: Chip, colors: ZeronColors, onTap: (Rect) -> Unit) {
    var bounds by remember { mutableStateOf(Rect.Zero) }
    val tone = when (chip.prState) {
        null -> null
        uniffi.zeron_core.PullRequestState.MERGED -> colors.accent
        uniffi.zeron_core.PullRequestState.CLOSED -> colors.danger
        else -> colors.success
    }
    val mono = chip.id == "branch" || chip.id == "pr"
    Row(
        Modifier
            .onGloballyPositioned { bounds = it.boundsInRoot() }
            .clip(RoundedCornerShape(14.dp))
            .background(tone?.copy(alpha = 0.1f) ?: colors.controlFill)
            .clickable { onTap(bounds) }
            .padding(horizontal = 10.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val fg = tone?.copy(alpha = 0.85f) ?: colors.text
        when (chip.id) {
            "model" -> BrandMark(chip.harness, colors, 13.dp)
            "effort" -> GaugeGlyph(fg, Modifier.size(13.dp))
            "pr" -> PrGlyph(fg, Modifier.size(12.dp))
            "branch" -> AssetIcon("tool-git-branch", 12.dp, fg)
            else -> Unit
        }
        if (chip.id in setOf("model", "effort", "pr", "branch")) Spacer(Modifier.width(if (chip.id == "pr") 5.dp else 6.dp))
        Text(
            chip.title,
            color = fg,
            fontFamily = if (mono) ZeronType.Mono else ZeronType.Sans,
            fontWeight = FontWeight.Medium,
            fontSize = if (mono) 13.sp else 14.sp,
            maxLines = 1,
        )
    }
}

@Composable
private fun ComposerBar(
    colors: ZeronColors,
    text: String,
    onText: (String) -> Unit,
    placeholder: String,
    running: Boolean,
    canSteer: Boolean,
    focused: Boolean,
    onFocus: (Boolean) -> Unit,
    chips: List<Chip>,
    onChip: (Chip, Rect) -> Unit = { _, _ -> },
    images: List<Staged>,
    onRemoveImage: (Staged) -> Unit,
    onAttach: () -> Unit = {},
    onSend: (Delivery) -> Unit,
    mentionSearch: suspend (String) -> List<uniffi.zeron_core.FileMatch>,
    onMention: (String, Boolean) -> Unit,
) {
    val resting = !focused && text.isEmpty() && images.isEmpty()
    val has = text.isNotBlank() || images.isNotEmpty()
    val stop = running && !has
    var deliveryMenu by remember { mutableStateOf(false) }
    var mentions by remember { mutableStateOf(listOf<uniffi.zeron_core.FileMatch>()) }
    val scope = rememberCoroutineScope()
    val keyboard = LocalSoftwareKeyboardController.current
    val card = !resting
    fun onTextChange(value: String) {
        onText(value)
        val query = Regex("@([^\\s]*)$").find(value)?.groupValues?.getOrNull(1)
        if (query != null) {
            scope.launch { mentions = mentionSearch(query).take(8) }
        } else mentions = emptyList()
    }
    Column(
        Modifier
            .fillMaxWidth()
            .glassSurface(colors, if (resting) 25.dp else 26.dp)
            .then(if (resting) Modifier.height(50.dp) else Modifier)
            .padding(horizontal = if (resting) 14.dp else 8.dp, vertical = if (resting) 0.dp else 6.dp),
        verticalArrangement = if (resting) Arrangement.Center else Arrangement.Top,
    ) {
        if (mentions.isNotEmpty()) {
            Column(Modifier.heightIn(max = 180.dp).verticalScroll(rememberScrollState())) {
                mentions.forEach { match ->
                    Text(
                        match.path,
                        color = colors.text,
                        fontFamily = ZeronType.Sans,
                        fontSize = 14.sp,
                        maxLines = 1,
                        modifier = Modifier.fillMaxWidth().clickable {
                            onMention(match.path, match.isDir)
                            mentions = emptyList()
                        }.padding(8.dp),
                    )
                }
            }
        }
        if (images.isNotEmpty()) {
            Row(Modifier.padding(bottom = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                images.forEach { staged ->
                    Box {
                        if (staged.preview != null) {
                            Image(
                                staged.preview.asImageBitmap(),
                                contentDescription = staged.name,
                                modifier = Modifier.size(56.dp).clip(RoundedCornerShape(10.dp)),
                                contentScale = ContentScale.Crop,
                            )
                        }
                        Text("×", color = Color.White, modifier = Modifier.align(Alignment.TopEnd).clickable { onRemoveImage(staged) }.padding(2.dp))
                    }
                }
            }
        }
        // One field for both states. Swapping two fields on focus dropped the
        // IME, because the focused instance left the composition.
        Box(Modifier.fillMaxWidth()) {
            BasicTextField(
                value = text,
                onValueChange = { onTextChange(it) },
                textStyle = TextStyle(color = colors.text, fontFamily = ZeronType.Sans, fontSize = 16.5.sp),
                cursorBrush = SolidColor(colors.accent),
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(
                        start = 8.dp,
                        end = if (card) 8.dp else 42.dp,
                        top = if (card) 6.dp else 0.dp,
                        bottom = if (card) 48.dp else 0.dp,
                    )
                    .onFocusChanged {
                        onFocus(it.isFocused)
                        if (it.isFocused) keyboard?.show()
                    },
                decorationBox = { inner ->
                    Box {
                        if (text.isEmpty()) Text(placeholder, color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 16.5.sp)
                        inner()
                    }
                },
                maxLines = if (card) 8 else 1,
            )
            if (card) {
                Row(
                    Modifier.align(Alignment.BottomCenter).fillMaxWidth().height(44.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(
                        Modifier.size(34.dp).clip(RoundedCornerShape(17.dp)).background(colors.controlFill).clickable(onClick = onAttach),
                        contentAlignment = Alignment.Center,
                    ) {
                        PlusMark(colors.text, Modifier.size(16.dp))
                    }
                    Row(
                        Modifier.weight(1f).padding(horizontal = 8.dp).horizontalScroll(rememberScrollState()),
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        chips.forEach { chip ->
                            ChipView(chip, colors) { rect -> onChip(chip, rect) }
                        }
                    }
                    SendButton(colors, stop, has, running, canSteer, deliveryMenu, { deliveryMenu = it }, onSend)
                }
            } else {
                Box(Modifier.align(Alignment.CenterEnd)) {
                    SendButton(colors, stop, has, running, canSteer, deliveryMenu, { deliveryMenu = it }, onSend)
                }
            }
        }
    }
}

@Composable
private fun SendButton(
    colors: ZeronColors,
    stop: Boolean,
    has: Boolean,
    running: Boolean,
    canSteer: Boolean,
    deliveryMenu: Boolean,
    onMenu: (Boolean) -> Unit,
    onSend: (Delivery) -> Unit,
) {
    Box {
        Box(
            Modifier
                .size(34.dp)
                .clip(RoundedCornerShape(17.dp))
                .background(if (stop) colors.text else if (has) colors.accent else colors.text.copy(alpha = 0.10f))
                .combinedClickable(
                    onClick = { onSend(if (stop) Delivery.Send else if (running && canSteer) Delivery.Steer else Delivery.Queue) },
                    onLongClick = { if (running && has) onMenu(true) },
                ),
            contentAlignment = Alignment.Center,
        ) {
            if (stop) StopMark(colors.background, Modifier.size(16.dp)) else ArrowUpMark(if (has) Color.White else colors.tertiary, Modifier.size(16.dp))
        }
        if (running && has) {
            DropdownMenu(expanded = deliveryMenu, onDismissRequest = { onMenu(false) }) {
                DropdownMenuItem(text = { Text("Queue for next turn") }, onClick = { onMenu(false); onSend(Delivery.Queue) })
                if (canSteer) DropdownMenuItem(text = { Text("Steer now") }, onClick = { onMenu(false); onSend(Delivery.Steer) })
                DropdownMenuItem(text = { Text("Stop and send") }, onClick = { onMenu(false); onSend(Delivery.Interrupt) })
            }
        }
    }
}

@Composable
fun NewSessionSheet(model: ZeronModel, onDismiss: () -> Unit) {
    val colors = LocalZeronColors.current
    val client = model.client ?: return
    val scope = rememberCoroutineScope()
    var text by remember { mutableStateOf("") }
    var harness by remember { mutableStateOf("claude-code") }
    var modelId by remember { mutableStateOf<String?>(null) }
    var effort by remember { mutableStateOf<String?>(null) }
    var projectId by remember { mutableStateOf(client.projects().firstOrNull()?.id) }
    var focused by remember { mutableStateOf(true) }
    val projects = client.projects()
    val project = projects.firstOrNull { it.id == projectId }
    Column(Modifier.fillMaxSize().background(colors.background).padding(horizontal = 16.dp).windowInsetsPadding(WindowInsets.ime.union(WindowInsets.navigationBars))) {
        Row(Modifier.fillMaxWidth().padding(top = 8.dp), horizontalArrangement = Arrangement.SpaceBetween) {
            Text("Close", color = colors.text, modifier = Modifier.clickable(onClick = onDismiss).padding(8.dp), fontFamily = ZeronType.Sans)
            Text("New Session", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, modifier = Modifier.padding(8.dp))
            Spacer(Modifier.width(48.dp))
        }
        Column(Modifier.weight(1f, fill = false).fillMaxWidth().padding(vertical = 28.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            BrandMark(harness, colors, 34.dp)
            Spacer(Modifier.height(14.dp))
            Text("What are we building?", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 22.sp)
        }
        ComposerBar(
            colors = colors,
            text = text,
            onText = { text = it },
            placeholder = "Describe the task",
            running = false,
            canSteer = false,
            focused = true,
            onFocus = { focused = it },
            chips = listOfNotNull(
                Chip("project", project?.name ?: "No project"),
                Chip("model", modelId?.let { modelLabel(harness, it) } ?: harnessLabel(harness), harness = harness),
                effort?.let { Chip("effort", reasoningLabel(it)) },
            ),
            images = emptyList(),
            onRemoveImage = {},
            onSend = send@{
                val body = text.trim()
                if (body.isEmpty()) return@send
                try {
                    val target = if (project != null) SessionTarget.Project(project.id) else {
                        val host = client.executionDevices().firstOrNull()?.id ?: client.deviceId()
                        SessionTarget.Projectless(host)
                    }
                    val id = client.createSession(NewSession(target, model.defaultConfig(harness, modelId, effort), null, null, null))
                    val handle = client.openSession(id)
                    handle.send(SendRequest(body, emptyList(), null, BusyPolicy.QUEUE))
                    handle.close()
                    onDismiss()
                    model.openSession(id)
                } catch (t: Throwable) {
                    model.showToast(t.message ?: "Couldn't start the session")
                }
            },
            mentionSearch = search@{ q ->
                val p = project ?: return@search emptyList()
                runCatching { client.searchFiles(p.deviceId, null, p.id, q) }.getOrDefault(emptyList())
            },
            onMention = { path, dir -> text = text.replace(Regex("@[^\\s]*$"), fileMentionLink(path, dir) + " ") },
        )
        Spacer(Modifier.height(12.dp))
        // Project / harness pickers live under the composer, matching the chip row's menus.
        if (projects.isNotEmpty()) {
            Text("Project", color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 12.sp, modifier = Modifier.padding(top = 8.dp))
            LazyColumn(Modifier.heightIn(max = 160.dp)) {
                items(projects, key = { it.id }) { p ->
                    Row(
                        Modifier.fillMaxWidth().clickable { projectId = p.id }.padding(vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        ProjectTile(p.name, p.colorIndex.toInt(), colors, 16.dp)
                        Spacer(Modifier.width(8.dp))
                        Text(p.name, color = if (p.id == projectId) colors.text else colors.secondary, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium)
                        if (!p.deviceOnline) Text("  offline", color = colors.tertiary, fontSize = 12.sp)
                    }
                }
            }
        }
        LaunchedEffect(project?.deviceId, harness) {
            val device = project?.deviceId ?: client.executionDevices().firstOrNull()?.id ?: return@LaunchedEffect
            val models = runCatching { client.listModels(device, harness) }.getOrDefault(emptyList())
            if (modelId == null) modelId = models.firstOrNull()?.id
            if (effort == null) effort = models.firstOrNull()?.defaultReasoning
        }
    }
}

@Composable
fun SignInScreen(model: ZeronModel) {
    val colors = LocalZeronColors.current
    val context = LocalContext.current
    Column(
        Modifier.fillMaxSize().background(colors.background).statusBarsPadding().navigationBarsPadding().padding(24.dp),
        verticalArrangement = Arrangement.SpaceBetween,
    ) {
        Spacer(Modifier.height(1.dp))
        Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.fillMaxWidth()) {
            Text("✦", color = colors.text, fontSize = 44.sp)
            Spacer(Modifier.height(12.dp))
            Text("Zeron", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 34.sp)
            Spacer(Modifier.height(8.dp))
            Text("Your coding agents, from anywhere.", color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 17.sp)
        }
        Column {
            model.signInError?.let { Text(it, color = colors.danger, fontFamily = ZeronType.Sans, fontSize = 14.sp, modifier = Modifier.padding(bottom = 12.dp)) }
            model.authOrgs?.let { orgs ->
                Text("Choose an organization", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(bottom = 8.dp))
                orgs.forEach { org ->
                    Text(org.name, color = colors.text, modifier = Modifier.fillMaxWidth().clickable { model.chooseOrg(org) }.padding(vertical = 10.dp), fontFamily = ZeronType.Sans, fontSize = 16.sp)
                }
            }
            Box(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(28.dp)).background(colors.text).clickable {
                    val url = model.authorizeUrl()
                    val tabs = androidx.browser.customtabs.CustomTabsIntent.Builder().build()
                    tabs.launchUrl(context, android.net.Uri.parse(url))
                }.padding(vertical = 15.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(if (model.signInBusy) "Signing in…" else "Sign In", color = colors.background, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
            }
            Spacer(Modifier.height(12.dp))
            Box(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(28.dp)).background(colors.controlFill).clickable { model.enterDemo() }.padding(vertical = 15.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text("Explore the demo", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 17.sp)
            }
            Spacer(Modifier.height(12.dp))
            Box(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(28.dp)).background(colors.controlFill).clickable { model.showMachines = true }.padding(vertical = 15.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text("Connect to your computer (SSH)", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 17.sp)
            }
        }
    }
}

private val ChatIndicator.word: String?
    get() = when (this) {
        ChatIndicator.WORKING -> "Working"
        ChatIndicator.AWAITING_INPUT -> "Input"
        ChatIndicator.ERRORED -> "Failed"
        ChatIndicator.COMPLETED -> "Done"
        ChatIndicator.IDLE -> null
    }
