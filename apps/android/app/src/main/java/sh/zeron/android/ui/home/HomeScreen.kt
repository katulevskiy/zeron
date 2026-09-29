package sh.zeron.android.ui.home

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.Formatters
import sh.zeron.android.core.SessionLists
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.GlyphIcon
import sh.zeron.android.design.GlyphKind
import sh.zeron.android.design.LocalColors
import sh.zeron.android.design.StatusGlyph
import sh.zeron.android.design.SvgIcon
import sh.zeron.android.design.ZText
import sh.zeron.android.design.ZType
import sh.zeron.android.design.glass
import sh.zeron.android.design.pressable
import sh.zeron.android.ui.LocalModel
import sh.zeron.android.ui.LocalNavigator
import sh.zeron.android.ui.Route
import sh.zeron.android.ui.Tab
import sh.zeron.android.ui.project.ProjectsTab
import sh.zeron.android.ui.sessions.SessionsTab
import sh.zeron.android.ui.settings.SettingsTab

/** Height of the floating bottom chrome (capsule + tab bar), above the nav bar. */
private val ChromeHeight = 52.dp + 10.dp + 60.dp + 12.dp

/**
 * The root: three tabs under floating chrome — a "New session" capsule with
 * the live summary (iOS's tab accessory) over a glass tab bar. Tabs keep
 * their scroll state while switched away.
 */
@Composable
fun HomeScreen() {
    val nav = LocalNavigator.current
    val c = LocalColors.current
    val holder = rememberSaveableStateHolder()
    val navBottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
    val listBottom = ChromeHeight + navBottom + 12.dp
    Box(Modifier.fillMaxSize()) {
        holder.SaveableStateProvider(nav.tab.name) {
            when (nav.tab) {
                Tab.Sessions -> SessionsTab(listBottom)
                Tab.Projects -> ProjectsTab(listBottom)
                Tab.Settings -> SettingsTab(listBottom)
            }
        }
        // Lists fade into the background behind the chrome.
        Box(
            Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .height(ChromeHeight + navBottom + 24.dp)
                .background(Brush.verticalGradient(0f to Color.Transparent, 0.35f to c.background.copy(alpha = 0.94f), 0.6f to c.background)),
        )
        Column(
            Modifier
                .align(Alignment.BottomCenter)
                .windowInsetsPadding(WindowInsets.navigationBars)
                .padding(horizontal = 16.dp)
                .padding(bottom = 12.dp)
                .widthIn(max = 560.dp)
                .fillMaxWidth(),
        ) {
            NewSessionCapsule { nav.push(Route.NewSession()) }
            Spacer(Modifier.height(10.dp))
            TabBar(nav.tab) { nav.tab = it }
        }
    }
}

/** "+ New session" with "2 working · 1 needs you" on the right. */
@Composable
private fun NewSessionCapsule(onClick: () -> Unit) {
    val model = LocalModel.current
    val c = LocalColors.current
    val ws by model.workspace.collectAsState()
    val (working, awaiting) = ws?.let { SessionLists.liveCounts(it.front) } ?: (0 to 0)
    val summary = Formatters.liveSummary(working, awaiting)
    Row(
        Modifier
            .fillMaxWidth()
            .height(52.dp)
            .glass(c, CircleShape, 10.dp)
            .pressable(shape = CircleShape, label = "New session", onClick = onClick)
            .semantics { contentDescription = if (summary.isEmpty()) "New session" else "New session, $summary" }
            .padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier
                .size(36.dp)
                .background(c.accentSoft, CircleShape),
            contentAlignment = Alignment.Center,
        ) { GlyphIcon(Glyph.Plus, c.accent, size = 18.dp, weight = 2.2f) }
        Spacer(Modifier.width(11.dp))
        ZText("New session", ZType.sans(16f, FontWeight.Medium), c.text, Modifier.weight(1f), maxLines = 1)
        AnimatedVisibility(summary.isNotEmpty(), enter = fadeIn(), exit = fadeOut()) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                StatusGlyph(if (working > 0) GlyphKind.Spinner else GlyphKind.Dot(c.statusInput))
                Spacer(Modifier.width(7.dp))
                ZText(summary, ZType.sans(13f, FontWeight.Medium), c.secondary, maxLines = 1)
                Spacer(Modifier.width(10.dp))
            }
        }
    }
}

@Composable
private fun TabBar(selected: Tab, onSelect: (Tab) -> Unit) {
    val c = LocalColors.current
    Row(
        Modifier
            .fillMaxWidth()
            .height(60.dp)
            .glass(c, CircleShape, 12.dp)
            .padding(5.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        for (tab in Tab.entries) {
            val on = tab == selected
            val tint by animateColorAsState(if (on) c.text else c.secondary, label = "tab")
            val fill by animateColorAsState(if (on) c.controlFill else Color.Transparent, label = "tabfill")
            Column(
                Modifier
                    .weight(1f)
                    .fillMaxHeight()
                    .background(fill, CircleShape)
                    .pressable(shape = CircleShape, dim = 0.7f, label = tab.name) { onSelect(tab) },
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                when (tab) {
                    Tab.Sessions -> SvgIcon("tab-chat", 22.dp, tint)
                    Tab.Projects -> GlyphIcon(Glyph.Folder, tint, size = 22.dp)
                    Tab.Settings -> SvgIcon("tab-settings", 22.dp, tint)
                }
                Spacer(Modifier.height(2.dp))
                ZText(tab.name, ZType.sans(11f, if (on) FontWeight.SemiBold else FontWeight.Medium), tint, maxLines = 1)
            }
        }
    }
}
