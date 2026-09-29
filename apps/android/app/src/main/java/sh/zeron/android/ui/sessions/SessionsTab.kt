package sh.zeron.android.ui.sessions

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsTopHeight
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.ListItem
import sh.zeron.android.core.SessionLists
import sh.zeron.android.design.Anchor
import sh.zeron.android.design.CircleIconButton
import sh.zeron.android.design.EmptyState
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.GlyphKind
import sh.zeron.android.design.LargeTitle
import sh.zeron.android.design.LocalColors
import sh.zeron.android.design.LocalOverlays
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.Overlays
import sh.zeron.android.design.StatusGlyph
import sh.zeron.android.design.ZText
import sh.zeron.android.design.ZType
import sh.zeron.android.design.anchor
import sh.zeron.android.design.rememberAnchor
import sh.zeron.android.ui.LocalModel
import sh.zeron.android.ui.LocalNavigator
import sh.zeron.android.ui.Navigator
import sh.zeron.android.ui.Route
import uniffi.zeron_core.ConnectivityState

/**
 * Front page, laid out like the desktop sidebar: Pinned and the user's
 * sections as foldable groups, then Recent. Section folds sync with the
 * desktop; Pinned/Recent fold locally. Long-press a header for its actions.
 */
@Composable
fun SessionsTab(bottomPadding: Dp) {
    val model = LocalModel.current
    val nav = LocalNavigator.current
    val overlays = LocalOverlays.current
    val c = LocalColors.current
    val ws by model.workspace.collectAsState()
    var localCollapsed by remember { mutableStateOf(model.prefs.collapsedSections) }
    val items = remember(ws, localCollapsed) { ws?.let { SessionLists.frontPage(it.front, localCollapsed) } }
    val options = rememberAnchor()

    fun toggle(h: ListItem.Header) {
        if (h.id == SessionLists.PINNED || h.id == SessionLists.RECENT) {
            localCollapsed = if (h.collapsed) localCollapsed - h.id else localCollapsed + h.id
            model.prefs.collapsedSections = localCollapsed
        } else {
            model.setSectionCollapsed(h.id, !h.collapsed)
        }
    }

    val top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
    Box(Modifier.fillMaxSize()) {
        LazyColumn(
            Modifier.fillMaxSize(),
            contentPadding = PaddingValues(top = top, bottom = bottomPadding),
        ) {
            item("title") {
                LargeTitle("Sessions") {
                    CircleIconButton(Glyph.Search, label = "Search") { nav.push(Route.Search) }
                    CircleIconButton(Glyph.Ellipsis, Modifier.anchor(options), label = "Options") {
                        frontPageOptions(model, overlays, nav, options, items.orEmpty()) { ids ->
                            localCollapsed = localCollapsed + ids.filter { it == SessionLists.PINNED || it == SessionLists.RECENT }
                            model.prefs.collapsedSections = localCollapsed
                        }
                    }
                }
            }
            item("connectivity") { ConnectivityBanner(model) }
            when {
                items == null -> item("loading") {
                    Box(Modifier.fillMaxWidth().padding(top = 120.dp), contentAlignment = Alignment.Center) {
                        StatusGlyph(GlyphKind.Spinner, size = 18.dp)
                    }
                }
                items.isEmpty() -> item("empty") {
                    val detail = if (model.mode.value == sh.zeron.android.core.AppMode.Phone) {
                        "Start one with New session below — the agent runs right here on this phone."
                    } else {
                        "Start one with New session below — agents run on your devices and report back here."
                    }
                    EmptyState(Glyph.Chat, "No sessions yet", detail, Modifier.fillMaxWidth().padding(top = 60.dp))
                }
                else -> items(items, key = { it.key }, contentType = { it::class }) { item ->
                    when (item) {
                        is ListItem.Header -> {
                            val anchor = rememberAnchor()
                            Box(Modifier.animateItem().anchor(anchor)) {
                                SectionHeaderView(item, onToggle = { toggle(item) }, onLongClick = headerMenu(model, overlays, nav, anchor, item))
                            }
                        }
                        is ListItem.Session -> Box(Modifier.animateItem()) {
                            SessionListRow(item.row, model, overlays) { nav.push(Route.Session(item.row.id)) }
                        }
                    }
                }
            }
        }
        // Content scrolls under a soft status-bar scrim (edge-to-edge).
        Box(
            Modifier
                .fillMaxWidth()
                .windowInsetsTopHeight(WindowInsets.statusBars)
                .background(Brush.verticalGradient(listOf(c.background, c.background.copy(alpha = 0.85f)))),
        )
    }
}

private fun headerMenu(model: AppModel, overlays: Overlays, nav: Navigator, anchor: Anchor, h: ListItem.Header): (() -> Unit)? {
    if (h.id == SessionLists.RECENT) return null
    return {
        overlays.menu(anchor, h.title, buildList {
            add(MenuEntry.Item("Open", glyph = Glyph.ArrowUpRight) { nav.push(Route.Folder(h.id, h.title)) })
            if (h.id == SessionLists.PINNED) {
                add(MenuEntry.Item("Reorder…", glyph = Glyph.Stack) { nav.push(Route.Folder(h.id, h.title)) })
            } else {
                add(MenuEntry.Item("Rename Section…", glyph = Glyph.Pencil) {
                    overlays.prompt("Rename Section", initial = h.title, action = "Rename") { model.renameSection(h.id, it) }
                })
                add(MenuEntry.Item("Delete Section", glyph = Glyph.Trash, destructive = true) {
                    overlays.confirm("Delete “${h.title}”?", "Its sessions move back to the main list.", "Delete", destructive = true) { model.deleteSection(h.id) }
                })
            }
        })
    }
}

private fun frontPageOptions(model: AppModel, overlays: Overlays, nav: Navigator, anchor: Anchor, items: List<ListItem>, collapseLocal: (List<String>) -> Unit) {
    overlays.menu(anchor, null, listOf(
        MenuEntry.Item("New Section…", glyph = Glyph.FolderPlus) {
            overlays.prompt("New Section", placeholder = "Name", action = "Create") { model.createSection(it) }
        },
        MenuEntry.Item("Collapse All", glyph = Glyph.ChevronDown) {
            val headers = items.filterIsInstance<ListItem.Header>().filter { !it.collapsed }
            headers.filter { it.id != SessionLists.PINNED && it.id != SessionLists.RECENT }.forEach { model.setSectionCollapsed(it.id, true) }
            collapseLocal(headers.map { it.id })
        },
        MenuEntry.Item("Archived", glyph = Glyph.Archive) { nav.push(Route.Folder(SessionLists.ARCHIVED, "Archived")) },
        MenuEntry.Item("Refresh", glyph = Glyph.Refresh) { model.refreshNow() },
    ))
}

/** "Offline" / "Reconnecting in 4s" under the title while the edge is away. */
@Composable
fun ConnectivityBanner(model: AppModel) {
    val c = LocalColors.current
    val conn by model.connectivity.collectAsState()
    val text = when (conn?.state) {
        ConnectivityState.OFFLINE -> "Offline — changes sync when you're back"
        ConnectivityState.RECONNECTING -> "Reconnecting…"
        else -> null
    }
    AnimatedVisibility(text != null, enter = fadeIn() + expandVertically(), exit = fadeOut() + shrinkVertically()) {
        Row(
            Modifier
                .padding(start = 20.dp, top = 2.dp, bottom = 8.dp)
                .background(c.controlFill, CircleShape)
                .padding(horizontal = 12.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            StatusGlyph(GlyphKind.Dot(c.statusIdle.copy(alpha = 0.5f)), size = 10.dp)
            Spacer(Modifier.width(6.dp))
            ZText(text ?: "", ZType.sans(13f, FontWeight.Medium), c.secondary, maxLines = 1)
        }
    }
    Spacer(Modifier.height(2.dp))
}
