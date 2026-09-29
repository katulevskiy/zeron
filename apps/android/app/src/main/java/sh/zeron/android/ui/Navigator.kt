package sh.zeron.android.ui

import androidx.activity.compose.PredictiveBackHandler
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import sh.zeron.android.design.LocalColors

/** Every screen the app can push. */
sealed interface Route {
    val key: String

    data object Home : Route { override val key = "home" }
    data class Session(val chatId: String) : Route { override val key = "session:$chatId" }
    /** Pinned, a user section, or Archived. */
    data class Folder(val id: String, val name: String) : Route { override val key = "folder:$id" }
    data class Project(val id: String) : Route { override val key = "project:$id" }
    data class NewSession(val projectId: String? = null) : Route { override val key = "new-session" }
    data class NewProject(val forNewSession: Boolean = false) : Route { override val key = "new-project" }
    data object Search : Route { override val key = "search" }
    data object Runtime : Route { override val key = "runtime" }
    data object Agents : Route { override val key = "agents" }
}

enum class Tab { Sessions, Projects, Settings }

/**
 * A plain back stack. Screens push/pop routes; the host animates them as
 * horizontal slides and drives the top screen with the system's predictive
 * back gesture.
 */
@Stable
class Navigator {
    internal val stack = mutableStateListOf<Entry>(Entry(Route.Home))
    var tab by mutableStateOf(Tab.Sessions)

    /** Route results (a created project id for the new-session canvas). */
    var projectCreated by mutableStateOf<String?>(null)

    internal class Entry(val route: Route, pushed: Boolean = false) {
        // Pushed screens start off-screen: the host slides them in.
        val offset = Animatable(if (pushed) 100_000f else 0f)
        var entering by mutableStateOf(false)
        var leaving by mutableStateOf(false)
    }

    val top: Route get() = stack.last().route
    val depth get() = stack.size

    fun push(route: Route) {
        if (stack.last().route == route) return
        stack.add(Entry(route, pushed = true).apply { entering = true })
    }

    /** Pop (animated by the host). */
    fun pop() {
        if (stack.size <= 1) return
        stack.last().leaving = true
    }

    /** Replace the top without an exit animation (new session → the session). */
    fun replaceTop(route: Route) {
        if (stack.size > 1) stack.removeAt(stack.size - 1)
        push(route)
    }

    fun popToRoot() {
        while (stack.size > 1) stack.removeAt(stack.size - 1)
    }

    fun openSession(chatId: String) {
        popToRoot()
        tab = Tab.Sessions
        push(Route.Session(chatId))
    }

    internal fun remove(entry: Entry) {
        stack.remove(entry)
    }
}

val LocalNavigator = staticCompositionLocalOf { Navigator() }

@Composable
fun StackHost(nav: Navigator, content: @Composable (Route) -> Unit) {
    val holder = rememberSaveableStateHolder()
    val scope = rememberCoroutineScope()
    val colors = LocalColors.current
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val width = constraints.maxWidth.toFloat()
        val top = nav.stack.last()
        val below = nav.stack.getOrNull(nav.stack.size - 2)

        // Predictive back: the top screen follows the finger; release pops.
        // Open overlays (menus, sheets) take back first: the overlay host's
        // handler may be registered earlier than this one, so step aside.
        val overlays = sh.zeron.android.design.LocalOverlays.current
        PredictiveBackHandler(enabled = nav.stack.size > 1 && !top.leaving && !overlays.isShowing) { progress ->
            try {
                progress.collect { e -> top.offset.snapTo(e.progress * width * 0.9f) }
                top.offset.animateTo(width, tween(180))
                nav.remove(top)
            } catch (c: CancellationException) {
                scope.launch { top.offset.animateTo(0f, spring(stiffness = 900f)) }
                throw c
            }
        }

        LaunchedEffect(top, top.entering) {
            if (top.entering) {
                top.offset.snapTo(width)
                top.offset.animateTo(0f, spring(dampingRatio = 1f, stiffness = 520f))
                top.entering = false
            }
        }
        LaunchedEffect(top, top.leaving) {
            if (top.leaving) {
                top.offset.animateTo(width, tween(220))
                nav.remove(top)
            }
        }

        // The screen underneath shows only while the top one moves. One keyed
        // loop, so a screen keeps its composition moving between the slots.
        val showBelow = below != null && (top.entering || top.leaving || top.offset.value > 0.5f)
        val visible = if (showBelow) listOf(below!!, top) else listOf(top)
        for (entry in visible) {
            key(entry.route.key) {
                val isTop = entry === top
                Box(
                    Modifier
                        .fillMaxSize()
                        .graphicsLayer {
                            if (isTop) {
                                translationX = entry.offset.value.coerceAtMost(width * 2)
                                shadowElevation = if (entry.offset.value > 0.5f) 24f else 0f
                            } else {
                                val parallax = (top.offset.value / width).coerceIn(0f, 1f)
                                translationX = -(1f - parallax) * width * 0.28f
                            }
                        }
                        .background(colors.background)
                        .drawWithContent {
                            drawContent()
                            if (!isTop) {
                                val parallax = (top.offset.value / width).coerceIn(0f, 1f)
                                drawRect(Color.Black.copy(alpha = 0.18f * (1f - parallax)))
                            }
                        },
                ) { holder.SaveableStateProvider(entry.route.key) { content(entry.route) } }
            }
        }
    }
}
