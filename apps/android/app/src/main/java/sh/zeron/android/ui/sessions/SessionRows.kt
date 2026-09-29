package sh.zeron.android.ui.sessions

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.Corner
import sh.zeron.android.core.ListItem
import sh.zeron.android.core.LiveMark
import sh.zeron.android.core.SessionLists
import sh.zeron.android.design.Anchor
import sh.zeron.android.design.BrandMark
import sh.zeron.android.design.FadingText
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.GlyphIcon
import sh.zeron.android.design.GlyphKind
import sh.zeron.android.design.LocalColors
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.Overlays
import sh.zeron.android.design.PrBadge
import sh.zeron.android.design.PrState
import sh.zeron.android.design.ProjectTile
import sh.zeron.android.design.StatusGlyph
import sh.zeron.android.design.SvgIcon
import sh.zeron.android.design.ZText
import sh.zeron.android.design.ZType
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.anchor
import sh.zeron.android.design.pressable
import sh.zeron.android.design.rememberAnchor
import sh.zeron.android.design.typeFactor
import uniffi.zeron_core.PullRequestState
import uniffi.zeron_core.SessionRow
import uniffi.zeron_core.projectColorIndex
import kotlin.math.abs
import kotlin.math.roundToInt

/** Tone + glyph for a row's corner (desktop sidebar `status_dot_color`). */
fun cornerTone(corner: Corner, c: ZeronColors): Pair<Color, GlyphKind> = when (corner) {
    Corner.SendFailed -> c.danger to GlyphKind.Dot(c.danger)
    Corner.Working -> c.statusWorking.copy(alpha = 0.9f) to GlyphKind.Spinner
    Corner.Input -> c.accent to GlyphKind.Dot(c.statusInput)
    Corner.Failed -> c.statusFailed to GlyphKind.Dot(c.statusFailed)
    Corner.Done -> c.statusDone to GlyphKind.Check(c.statusDone)
}

/**
 * Two-line session row, fixed height (scales with font size, never
 * self-sizes):
 *
 *   [mark]  Title of the session ··········· ⠿ Working
 *           [P] project  ⎇ branch ··········· ⎇ 412
 */
@Composable
fun SessionRowView(
    row: SessionRow,
    modifier: Modifier = Modifier,
    current: Boolean = false,
    onLongClick: (() -> Unit)? = null,
    onClick: () -> Unit,
) {
    val c = LocalColors.current
    val k = typeFactor()
    val corner = SessionLists.corner(row)
    val emphasized = row.unseen || corner != null
    Box(
        modifier
            .fillMaxWidth()
            .height((62 * k).dp)
            .padding(horizontal = 8.dp, vertical = 1.dp)
            .clip(RoundedCornerShape(16.dp))
            .background(if (current) c.rowActive else Color.Transparent)
            .pressable(highlight = c.controlFill, dim = 1f, shape = RoundedCornerShape(16.dp), onLongClick = onLongClick, label = row.title, onClick = onClick)
            .semantics { contentDescription = listOfNotNull(row.title, row.project?.name, corner?.word).joinToString(", ") }
            .padding(horizontal = 12.dp),
    ) {
        Row(Modifier.padding(top = (10 * k).dp).height((22 * k).dp), verticalAlignment = Alignment.CenterVertically) {
            BrandMark(row.harness ?: "claude-code", 20.dp)
            Spacer(Modifier.width(14.dp))
            FadingText(
                row.title,
                ZType.sans(16.5f, if (row.unseen) FontWeight.SemiBold else FontWeight.Medium),
                if (emphasized) c.text else c.text.copy(alpha = 0.88f),
                Modifier.weight(1f),
            )
            Spacer(Modifier.width(10.dp))
            if (corner != null) {
                val (tone, glyph) = cornerTone(corner, c)
                StatusGlyph(glyph)
                Spacer(Modifier.width(5.dp))
                ZText(corner.word, ZType.sans(13f, FontWeight.Medium), tone, maxLines = 1)
            } else {
                ZText(row.timeLabel, ZType.sans(13f, FontWeight.Medium), c.statusTime, maxLines = 1)
            }
        }
        Row(
            Modifier
                .padding(start = 34.dp, top = (35 * k).dp)
                .height((18 * k).dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            val project = row.project
            ProjectTile(project?.name ?: "Home", (project?.colorIndex ?: projectColorIndex("home")).toInt(), size = (14 * k).dp)
            Spacer(Modifier.width(7.dp))
            Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                ZText(project?.name ?: row.deviceName ?: "No project", ZType.sans(13.5f), c.secondary, maxLines = 1)
                val branch = row.branch
                if (!branch.isNullOrEmpty() && row.pullRequest == null) {
                    Spacer(Modifier.width(10.dp))
                    SvgIcon("tool-git-branch", (12 * k).dp, c.subline)
                    Spacer(Modifier.width(3.dp))
                    FadingText(branch, ZType.sans(12.5f), c.subline)
                }
            }
            row.pullRequest?.let { pr ->
                Spacer(Modifier.width(10.dp))
                PrBadge(pr.number, pr.state.toPrState(), fontSize = 11f)
            }
        }
    }
}

fun PullRequestState.toPrState() = when (this) {
    PullRequestState.OPEN -> PrState.Open
    PullRequestState.MERGED -> PrState.Merged
    PullRequestState.CLOSED -> PrState.Closed
}

/** Foldable section header (desktop sidebar): name, count, live glyph, chevron. */
@Composable
fun SectionHeaderView(h: ListItem.Header, onToggle: () -> Unit, onLongClick: (() -> Unit)?) {
    val c = LocalColors.current
    val k = typeFactor()
    val angle by animateFloatAsState(if (h.collapsed) -90f else 0f, spring(dampingRatio = 0.85f, stiffness = 500f), label = "chevron")
    Row(
        Modifier
            .fillMaxWidth()
            .height((40 * k).dp)
            .padding(horizontal = 8.dp, vertical = 2.dp)
            .pressable(highlight = c.controlFill, dim = 1f, shape = RoundedCornerShape(12.dp), onLongClick = onLongClick, onClick = onToggle)
            .padding(start = 12.dp, end = 12.dp, bottom = 4.dp),
        verticalAlignment = Alignment.Bottom,
    ) {
        Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
            ZText(h.title, ZType.sans(13.5f, FontWeight.SemiBold), c.secondary, maxLines = 1)
            Spacer(Modifier.width(7.dp))
            ZText("${h.count}", ZType.sans(13.5f, FontWeight.Medium), c.tertiary, maxLines = 1)
            if (h.live != null) {
                Spacer(Modifier.width(7.dp))
                StatusGlyph(if (h.live == LiveMark.Working) GlyphKind.Spinner else GlyphKind.Dot(c.statusInput))
            }
        }
        GlyphIcon(Glyph.ChevronDown, c.tertiary, size = 14.dp, modifier = Modifier.rotate(angle), weight = 2.2f)
    }
}

/**
 * Horizontal swipe actions for a row: leading (swipe right) and trailing
 * (swipe left). Past the threshold the action runs when the finger lifts;
 * the row springs back either way.
 */
@Composable
fun SwipeRow(
    leading: SwipeAction?,
    trailing: SwipeAction?,
    content: @Composable () -> Unit,
) {
    val c = LocalColors.current
    val density = LocalDensity.current
    val threshold = with(density) { 88.dp.toPx() }
    val offset = remember { Animatable(0f) }
    val scope = rememberCoroutineScope()
    val haptics = LocalHapticFeedback.current
    // Haptic once per threshold crossing (not a state: no recomposition).
    val armed = remember { booleanArrayOf(false) }
    Box {
        val x = offset.value
        val action = if (x > 0) leading else if (x < 0) trailing else null
        if (action != null) {
            val past = abs(x) >= threshold
            Row(
                Modifier
                    .matchParentSize()
                    .padding(horizontal = 8.dp, vertical = 1.dp)
                    .clip(RoundedCornerShape(16.dp))
                    .background(action.tint.copy(alpha = if (past) 0.95f else 0.7f))
                    .padding(horizontal = 22.dp),
                horizontalArrangement = if (x > 0) Arrangement.Start else Arrangement.End,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                GlyphIcon(action.glyph, Color.White, size = 19.dp)
                Spacer(Modifier.width(8.dp))
                ZText(action.title, ZType.sans(14.5f, FontWeight.SemiBold), Color.White, maxLines = 1)
            }
        }
        Box(
            Modifier
                .offset { IntOffset(offset.value.roundToInt(), 0) }
                .background(if (offset.value != 0f) c.background else Color.Transparent)
                .draggable(
                    orientation = Orientation.Horizontal,
                    state = rememberDraggableState { delta ->
                        val next = offset.value + delta
                        val allowed = when {
                            next > 0 && leading == null -> 0f
                            next < 0 && trailing == null -> 0f
                            else -> next * if (abs(next) > threshold) 0.92f else 1f
                        }
                        scope.launch { offset.snapTo(allowed) }
                        val past = abs(allowed) >= threshold
                        if (past != armed[0]) {
                            armed[0] = past
                            if (past) haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                        }
                    },
                    onDragStopped = {
                        val v = offset.value
                        armed[0] = false
                        if (abs(v) >= threshold) (if (v > 0) leading else trailing)?.run?.invoke()
                        offset.animateTo(0f, spring(dampingRatio = 0.8f, stiffness = 500f))
                    },
                ),
        ) { content() }
    }
}

data class SwipeAction(val title: String, val glyph: Glyph, val tint: Color, val run: () -> Unit)

/** Long-press / ⋯ actions for a session row (shared by every list). */
fun sessionMenu(
    model: AppModel,
    overlays: Overlays,
    anchor: Anchor?,
    row: SessionRow,
    archived: Boolean,
    pinOrder: List<String>? = null,
): Unit = overlays.menu(anchor, row.title, buildList {
    val id = row.id
    if (archived) {
        add(MenuEntry.Item("Unarchive", glyph = Glyph.Unarchive) { model.unarchive(id) })
        add(MenuEntry.Item("Rename…", glyph = Glyph.Pencil) { renamePrompt(model, overlays, row) })
        add(MenuEntry.Divider)
        add(MenuEntry.Item("Delete…", glyph = Glyph.Trash, destructive = true) { deletePrompt(model, overlays, row) })
        return@buildList
    }
    add(MenuEntry.Item(if (row.pinned) "Unpin" else "Pin", glyph = Glyph.Pin) { model.setPinned(id, !row.pinned) })
    if (pinOrder != null) {
        SessionLists.pinMove(pinOrder, id, -1)?.let { (after, before) ->
            add(MenuEntry.Item("Move Up", glyph = Glyph.ArrowUp) { model.movePin(id, after, before) })
        }
        SessionLists.pinMove(pinOrder, id, 1)?.let { (after, before) ->
            add(MenuEntry.Item("Move Down", glyph = Glyph.ArrowDown) { model.movePin(id, after, before) })
        }
    }
    add(MenuEntry.Item("Move to Section…", glyph = Glyph.Folder) { moveMenu(model, overlays, anchor, row) })
    add(MenuEntry.Item("Rename…", glyph = Glyph.Pencil) { renamePrompt(model, overlays, row) })
    add(MenuEntry.Divider)
    add(MenuEntry.Item("Archive", glyph = Glyph.Archive) { archiveWithUndo(model, overlays, id) })
    add(MenuEntry.Item("Delete…", glyph = Glyph.Trash, destructive = true) { deletePrompt(model, overlays, row) })
})

fun archiveWithUndo(model: AppModel, overlays: Overlays, id: String) {
    model.archive(id)
    overlays.toast("Archived", "Undo") { model.unarchive(id) }
}

private fun moveMenu(model: AppModel, overlays: Overlays, anchor: Anchor?, row: SessionRow) {
    val sections = model.workspace.value?.front?.sections ?: emptyList()
    overlays.menu(anchor, "Move to Section", buildList {
        for (s in sections) add(MenuEntry.Item(s.name, glyph = Glyph.Folder, checked = row.sectionId == s.id) { model.moveToSection(row.id, s.id) })
        add(MenuEntry.Item("No Section", glyph = Glyph.Tray, checked = row.sectionId == null) { model.moveToSection(row.id, null) })
        add(MenuEntry.Divider)
        add(MenuEntry.Item("New Section…", glyph = Glyph.FolderPlus) {
            overlays.prompt("New Section", placeholder = "Name", action = "Create") { name ->
                model.createSection(name)
                // The section row lands with the next registry frame; assign once it exists.
                model.scope.launch {
                    repeat(20) {
                        val s = model.workspace.value?.front?.sections?.firstOrNull { it.name == name }
                        if (s != null) return@launch model.moveToSection(row.id, s.id)
                        kotlinx.coroutines.delay(150)
                    }
                }
            }
        })
    })
}

private fun renamePrompt(model: AppModel, overlays: Overlays, row: SessionRow) =
    overlays.prompt("Rename Session", initial = if (row.hasTitle) row.title else "", placeholder = "Title", action = "Rename") { model.rename(row.id, it) }

private fun deletePrompt(model: AppModel, overlays: Overlays, row: SessionRow) =
    overlays.confirm("Delete “${row.title}”?", "The session and its transcript are removed on every device.", "Delete", destructive = true) { model.delete(row.id) }

/** The standard swipeable, long-pressable session row. */
@Composable
fun SessionListRow(
    row: SessionRow,
    model: AppModel,
    overlays: Overlays,
    archived: Boolean = false,
    pinOrder: List<String>? = null,
    onOpen: () -> Unit,
) {
    val c = LocalColors.current
    val anchor = rememberAnchor()
    SwipeRow(
        leading = if (archived) null else SwipeAction(if (row.pinned) "Unpin" else "Pin", Glyph.Pin, c.accent) { model.setPinned(row.id, !row.pinned) },
        trailing = if (archived) SwipeAction("Unarchive", Glyph.Unarchive, c.accent) { model.unarchive(row.id) }
        else SwipeAction("Archive", Glyph.Archive, c.secondary) { archiveWithUndo(model, overlays, row.id) },
    ) {
        SessionRowView(
            row,
            Modifier.anchor(anchor),
            onLongClick = { sessionMenu(model, overlays, anchor, row, archived, pinOrder) },
            onClick = onOpen,
        )
    }
}

/** A placeholder list body while the first snapshot loads. */
@Composable
fun ListLoading(modifier: Modifier = Modifier) {
    Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        StatusGlyph(GlyphKind.Spinner, size = 18.dp)
    }
}

/** Small caps-free list caption ("3 sessions"). */
@Composable
fun ListCaption(text: String) {
    val c = LocalColors.current
    Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 10.dp)) {
        ZText(text, ZType.sans(13f, FontWeight.Medium), c.tertiary)
    }
}
