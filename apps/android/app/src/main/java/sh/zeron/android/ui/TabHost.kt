package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.withFrameNanos
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.zIndex
import androidx.compose.ui.semantics.hideFromAccessibility
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.flow.StateFlow

/**
 * [collectAsState] that stops listening while [active] is false and picks the flow's current value up again
 * (synchronously, in the same composition) when it turns true. A screen kept composed behind another one then
 * costs nothing while it is hidden, and is up to date the moment it is shown.
 */
@Composable
fun <T> StateFlow<T>.collectAsStateWhile(active: Boolean): T {
    // A plain holder, not snapshot state: it only carries the last value across the compositions that skip collecting.
    val last = remember(this) { arrayOfNulls<Any?>(1).also { it[0] = value } }
    if (active) {
        val live by collectAsState()
        last[0] = live
    }
    @Suppress("UNCHECKED_CAST")
    return last[0] as T
}

/**
 * One page of a tab host: composed and laid out whenever it is [ready], but painted, touched and announced only
 * while [active]. Showing it again is then a property change, not a composition (the 150-250 ms the Sessions and
 * Settings tabs used to cost on every switch). Until [ready] it is not composed at all and a [skeleton] stands in.
 */
@Composable
fun TabPage(active: Boolean, ready: Boolean, skeleton: @Composable () -> Unit, content: @Composable () -> Unit) {
    // Looping animations (spinners, breathing dots) run only while the page is shown, and start one frame after it
    // appears: a hidden page must not keep the display awake, and the switch itself should not pay for them.
    var settled by remember { mutableStateOf(active) }
    LaunchedEffect(active) {
        if (active) {
            if (!settled) withFrameNanos { }
            settled = true
        } else {
            // Not in the frame that hides the page: the switch has enough to do.
            withFrameNanos { }
            withFrameNanos { }
            settled = false
        }
    }
    CompositionLocalProvider(LocalMotionActive provides (active && settled)) {
        Box(
            Modifier
                .fillMaxSize()
                // The shown page sits on top (and swallows touches in its blank areas, below); a hidden one keeps its
                // recorded display lists and just stops being drawn: showing it is a property change, no recomposition.
                .zIndex(if (active) 1f else 0f)
                .graphicsLayer { alpha = if (active) 1f else 0f }
                .semantics { if (!active) hideFromAccessibility() }
                .then(if (active) Modifier.pointerInput(Unit) {} else Modifier),
        ) {
            if (ready) content() else if (active) skeleton()
        }
    }
}

/** False for a page that is hidden or has only just appeared; looping animations hold still until it is true. */
val LocalMotionActive = compositionLocalOf { true }

/** The wireframe tone: a quiet wash of the text colour, no animation (nothing here should cost a frame). */
@Composable
private fun wire(): Color = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.07f)

@Composable
private fun WireBar(width: Dp, height: Dp = 12.dp, modifier: Modifier = Modifier) {
    Box(modifier.size(width, height).clip(RoundedCornerShape(50)).background(wire()))
}

/** A session row's wireframe: the harness tile, a title and a project line, a status stub. */
@Composable
fun SessionRowSkeleton(modifier: Modifier = Modifier) {
    Row(
        modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp)
            .padding(bottom = 2.dp)
            .clip(RoundedCornerShape(20.dp))
            .background(cardColor())
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(48.dp).clip(RoundedCornerShape(16.dp)).background(wire()))
        Spacer(Modifier.width(16.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            WireBar(190.dp, 14.dp)
            WireBar(120.dp, 10.dp)
        }
        WireBar(36.dp, 12.dp)
    }
}

/** What the Sessions list shows before its first snapshot (and a tab shows for the frame before it is composed). */
@Composable
fun SessionRowsSkeleton(count: Int = 6) {
    Column {
        Spacer(Modifier.height(20.dp))
        Row(Modifier.padding(start = 28.dp, bottom = 12.dp)) { WireBar(72.dp, 12.dp) }
        repeat(count) { SessionRowSkeleton() }
    }
}

/** The whole Sessions tab as a wireframe. */
@Composable
fun SessionsSkeleton() {
    Column(Modifier.fillMaxSize().padding(top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 8.dp)) {
        ScreenHeader("Sessions", null)
        Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            for (w in listOf(48.dp, 92.dp, 84.dp, 72.dp)) Box(Modifier.size(w, 40.dp).clip(CircleShape).background(wire()))
        }
        SessionRowsSkeleton()
    }
}

/** The whole Settings tab as a wireframe. */
@Composable
fun SettingsSkeleton() {
    Column(Modifier.fillMaxSize().padding(top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 8.dp)) {
        ScreenHeader("Settings", null)
        Box(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp).height(104.dp).clip(RoundedCornerShape(32.dp)).background(wire()))
        Spacer(Modifier.height(16.dp))
        repeat(4) { SessionRowSkeleton() }
    }
}
