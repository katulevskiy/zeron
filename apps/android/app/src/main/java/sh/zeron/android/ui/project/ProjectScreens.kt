package sh.zeron.android.ui.project

import androidx.compose.foundation.background
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
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.layout.windowInsetsTopHeight
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.Formatters
import sh.zeron.android.design.CapsuleButton
import sh.zeron.android.design.CircleIconButton
import sh.zeron.android.design.EmptyState
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.GlyphIcon
import sh.zeron.android.design.GlyphKind
import sh.zeron.android.design.LargeTitle
import sh.zeron.android.design.LocalColors
import sh.zeron.android.design.LocalOverlays
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.ProjectTile
import sh.zeron.android.design.SectionLabel
import sh.zeron.android.design.StatusGlyph
import sh.zeron.android.design.TopBar
import sh.zeron.android.design.ZText
import sh.zeron.android.design.ZType
import sh.zeron.android.design.anchor
import sh.zeron.android.design.pressable
import sh.zeron.android.design.rememberAnchor
import sh.zeron.android.design.typeFactor
import sh.zeron.android.ui.LocalModel
import sh.zeron.android.ui.LocalNavigator
import sh.zeron.android.ui.Route
import sh.zeron.android.ui.sessions.SessionListRow
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.ProjectView
import uniffi.zeron_core.projectColorIndex

/** Projects, grouped by the device they live on; "No project" sessions last. */
@Composable
fun ProjectsTab(bottomPadding: Dp) {
    val model = LocalModel.current
    val nav = LocalNavigator.current
    val c = LocalColors.current
    val ws by model.workspace.collectAsState()
    val projects = ws?.projects ?: emptyList()
    val byDevice = projects.groupBy { it.deviceId }.toList().sortedBy { (_, list) -> list.first().deviceName ?: "" }
    val top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
    Box(Modifier.fillMaxSize()) {
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(top = top, bottom = bottomPadding)) {
            item("title") {
                LargeTitle("Projects") {
                    CircleIconButton(Glyph.Plus, label = "New project") { nav.push(Route.NewProject()) }
                }
            }
            if (ws != null && projects.isEmpty()) {
                item("empty") {
                    EmptyState(Glyph.Folder, "No projects yet", "A project is a folder on one of your devices. Agents work inside it.", Modifier.fillMaxWidth().padding(top = 40.dp)) {
                        CapsuleButton("New Project", glyph = Glyph.FolderPlus, compact = true) { nav.push(Route.NewProject()) }
                    }
                }
            }
            for ((_, list) in byDevice) {
                val first = list.first()
                item("device:${first.deviceId}") {
                    SectionLabel((first.deviceName ?: model.deviceName(first.deviceId)) + if (first.deviceOnline) "" else " · offline")
                }
                items(list, key = { it.id }) { p -> ProjectRow(p) { nav.push(Route.Project(p.id)) } }
            }
            val projectless = ws?.projectless ?: emptyList()
            if (projectless.isNotEmpty()) {
                item("projectless-label") { SectionLabel("Without a project") }
                item("projectless") {
                    FolderLikeRow("Home folder", "${projectless.size} session${if (projectless.size == 1) "" else "s"}", projectColorIndex("home").toInt()) {
                        nav.push(Route.Folder(PROJECTLESS, "Home folder"))
                    }
                }
            }
        }
        Box(
            Modifier
                .fillMaxWidth()
                .windowInsetsTopHeight(WindowInsets.statusBars)
                .background(Brush.verticalGradient(listOf(c.background, c.background.copy(alpha = 0.85f)))),
        )
    }
}

const val PROJECTLESS = "projectless"

@Composable
private fun ProjectRow(p: ProjectView, onClick: () -> Unit) {
    val c = LocalColors.current
    val live = p.sessions.any { it.indicator == ChatIndicator.WORKING }
    val asking = p.sessions.any { it.indicator == ChatIndicator.AWAITING_INPUT }
    val detail = buildList {
        add("${p.sessions.size} session${if (p.sessions.size == 1) "" else "s"}")
        add(p.path)
    }.joinToString(" · ")
    FolderLikeRow(p.name, detail, p.colorIndex.toInt(), onClick = onClick, trailing = {
        if (live || asking) {
            StatusGlyph(if (live) GlyphKind.Spinner else GlyphKind.Dot(c.statusInput))
            Spacer(Modifier.width(8.dp))
        }
        if (p.unseenCount > 0u) {
            Box(Modifier.background(c.accentSoft, CircleShape).padding(horizontal = 8.dp, vertical = 2.dp)) {
                ZText("${p.unseenCount}", ZType.sans(12f, FontWeight.SemiBold), c.accent)
            }
            Spacer(Modifier.width(8.dp))
        }
    })
}

@Composable
private fun FolderLikeRow(title: String, detail: String, colorIndex: Int, trailing: @Composable () -> Unit = {}, onClick: () -> Unit) {
    val c = LocalColors.current
    val k = typeFactor()
    Row(
        Modifier
            .fillMaxWidth()
            .height((60 * k).dp)
            .padding(horizontal = 8.dp, vertical = 1.dp)
            .pressable(highlight = c.controlFill, dim = 1f, shape = RoundedCornerShape(16.dp), label = title, onClick = onClick)
            .padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ProjectTile(title, colorIndex, size = 30.dp)
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            ZText(title, ZType.sans(16.5f, FontWeight.Medium), c.text, maxLines = 1)
            ZText(detail, ZType.sans(13f), c.secondary, maxLines = 1)
        }
        trailing()
        GlyphIcon(Glyph.ChevronRight, c.tertiary, size = 14.dp)
    }
}

/** One project's sessions, with rename/delete and "New session here". */
@Composable
fun ProjectScreen(id: String) {
    val model = LocalModel.current
    val nav = LocalNavigator.current
    val overlays = LocalOverlays.current
    val c = LocalColors.current
    val ws by model.workspace.collectAsState()
    val p = ws?.projects?.firstOrNull { it.id == id }
    val options = rememberAnchor()
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize()) {
            TopBar(p?.name ?: "Project", subtitle = p?.let { "${Formatters.lastComponent(it.path)} @ ${it.deviceName ?: model.deviceName(it.deviceId)}" }, onBack = { nav.pop() }) {
                if (p != null) {
                    CircleIconButton(Glyph.Ellipsis, Modifier.anchor(options), label = "Project options") {
                        overlays.menu(options, p.path, listOf(
                            MenuEntry.Item("New Session Here", glyph = Glyph.Plus) { nav.push(Route.NewSession(p.id)) },
                            MenuEntry.Item("Rename…", glyph = Glyph.Pencil) {
                                overlays.prompt("Rename Project", initial = p.name, action = "Rename") { model.renameProject(p.id, it) }
                            },
                            MenuEntry.Divider,
                            MenuEntry.Item("Remove Project…", glyph = Glyph.Trash, destructive = true) {
                                overlays.confirm("Remove “${p.name}”?", "The folder stays on ${p.deviceName ?: "the device"}; its sessions move to no project.", "Remove", destructive = true) {
                                    model.deleteProject(p.id)
                                    nav.pop()
                                }
                            },
                        ))
                    }
                }
            }
            val rows = p?.sessions ?: emptyList()
            if (p != null && rows.isEmpty()) {
                EmptyState(Glyph.Chat, "No sessions in ${p.name}", "Start one — the agent works in ${p.path}.", Modifier.fillMaxWidth().padding(top = 60.dp))
            }
            LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(top = 4.dp, bottom = 120.dp)) {
                items(rows, key = { it.id }) { row ->
                    Box(Modifier.animateItem()) { SessionListRow(row, model, overlays) { nav.push(Route.Session(row.id)) } }
                }
            }
        }
        if (p != null) {
            Box(
                Modifier
                    .align(Alignment.BottomCenter)
                    .fillMaxWidth()
                    .background(Brush.verticalGradient(listOf(c.background.copy(alpha = 0f), c.background)))
                    .windowInsetsPadding(WindowInsets.navigationBars)
                    .padding(horizontal = 20.dp, vertical = 14.dp),
            ) {
                CapsuleButton("New session in ${p.name}", Modifier.fillMaxWidth(), glyph = Glyph.Plus) { nav.push(Route.NewSession(p.id)) }
            }
        }
    }
}
