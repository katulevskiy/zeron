package sh.zeron.android.ui.sessions

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import sh.zeron.android.core.SessionLists
import sh.zeron.android.design.CircleIconButton
import sh.zeron.android.design.EmptyState
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.LocalOverlays
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.SearchField
import sh.zeron.android.design.TopBar
import sh.zeron.android.design.anchor
import sh.zeron.android.design.rememberAnchor
import sh.zeron.android.ui.LocalModel
import sh.zeron.android.ui.LocalNavigator
import sh.zeron.android.ui.Route
import uniffi.zeron_core.SessionRow

/** One folder's sessions: Pinned (reorderable from the row menu), a user section, or Archived. */
@Composable
fun FolderScreen(id: String, name: String) {
    val model = LocalModel.current
    val nav = LocalNavigator.current
    val overlays = LocalOverlays.current
    val ws by model.workspace.collectAsState()
    val archived = id == SessionLists.ARCHIVED
    val section = ws?.front?.sections?.firstOrNull { it.id == id }
    val rows: List<SessionRow> = when {
        ws == null -> emptyList()
        archived -> ws!!.archived
        id == SessionLists.PINNED -> ws!!.front.pinned
        id == sh.zeron.android.ui.project.PROJECTLESS -> ws!!.projectless
        else -> section?.sessions ?: emptyList()
    }
    val title = section?.name ?: name
    val options = rememberAnchor()
    Column(Modifier.fillMaxSize()) {
        TopBar(title, subtitle = "${rows.size} session${if (rows.size == 1) "" else "s"}", onBack = { nav.pop() }) {
            if (section != null) {
                CircleIconButton(Glyph.Ellipsis, Modifier.anchor(options), label = "Section options") {
                    overlays.menu(options, title, listOf(
                        MenuEntry.Item("Rename Section…", glyph = Glyph.Pencil) {
                            overlays.prompt("Rename Section", initial = title, action = "Rename") { model.renameSection(id, it) }
                        },
                        MenuEntry.Item("Delete Section", glyph = Glyph.Trash, destructive = true) {
                            overlays.confirm("Delete “$title”?", "Its sessions move back to the main list.", "Delete", destructive = true) {
                                model.deleteSection(id)
                                nav.pop()
                            }
                        },
                    ))
                }
            }
        }
        if (rows.isEmpty()) {
            EmptyState(
                if (archived) Glyph.Archive else Glyph.Folder,
                if (archived) "Nothing archived" else "No sessions here",
                if (archived) "Archived sessions stay here until you bring them back." else null,
                Modifier.fillMaxWidth().padding(top = 80.dp),
            )
        } else {
            val pinOrder = if (id == SessionLists.PINNED) rows.map { it.id } else null
            LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(top = 4.dp, bottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() + 16.dp)) {
                items(rows, key = { it.id }) { row ->
                    Box(Modifier.animateItem()) {
                        SessionListRow(row, model, overlays, archived = archived, pinOrder = pinOrder) { nav.push(Route.Session(row.id)) }
                    }
                }
            }
        }
    }
}

/** Search: live, core-ranked matches over titles, projects, branches and previews. */
@Composable
fun SearchScreen() {
    val model = LocalModel.current
    val nav = LocalNavigator.current
    val overlays = LocalOverlays.current
    val ws by model.workspace.collectAsState()
    var query by rememberSaveable { mutableStateOf("") }
    val results = remember(query, ws) { model.search(query) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) {
        delay(250) // after the push lands: the keyboard would fight the slide
        runCatching { focus.requestFocus() }
    }
    Column(Modifier.fillMaxSize()) {
        TopBar("Search", onBack = { nav.pop() })
        SearchField(query, { query = it }, "Sessions, projects, branches", Modifier.padding(horizontal = 16.dp, vertical = 6.dp), focus)
        if (results.isEmpty() && query.isNotBlank()) {
            EmptyState(Glyph.Search, "No matches", "Try a project, branch or a word from the conversation.", Modifier.fillMaxWidth().padding(top = 60.dp))
        } else {
            if (query.isBlank()) ListCaption("Recent")
            LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() + 16.dp)) {
                items(results, key = { it.id }) { row ->
                    SessionListRow(row, model, overlays, archived = row.archived) { nav.push(Route.Session(row.id)) }
                }
            }
        }
    }
}
