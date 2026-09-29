package sh.zeron.android.design

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.MutableTransitionState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

/** One row of a menu (context menus, chip pickers). */
sealed interface MenuEntry {
    data class Item(
        val title: String,
        val subtitle: String? = null,
        val glyph: Glyph? = null,
        val icon: (@Composable () -> Unit)? = null,
        val checked: Boolean = false,
        val destructive: Boolean = false,
        val enabled: Boolean = true,
        val onClick: () -> Unit,
    ) : MenuEntry

    data class Header(val title: String) : MenuEntry
    data object Divider : MenuEntry
}

/** Where an anchored overlay points (root coordinates). */
@Stable
class Anchor {
    var bounds by mutableStateOf<Rect?>(null)
}

@Composable
fun rememberAnchor() = remember { Anchor() }

fun Modifier.anchor(anchor: Anchor): Modifier = onGloballyPositioned { anchor.bounds = it.boundsInRoot() }

/**
 * App-wide overlay layer: menus, sheets, dialogs and the toast, drawn above
 * every screen inside the same window (so IME insets and theming match) and
 * styled as Zeron chrome rather than platform dialogs. Back dismisses the top.
 */
@Stable
class Overlays {
    internal class Entry(val key: Long, val kind: Kind, val content: @Composable (dismiss: () -> Unit) -> Unit) {
        val state = MutableTransitionState(false).apply { targetState = true }
        var anchor: Rect? = null
    }

    enum class Kind { Menu, Sheet, Dialog, Viewer }

    internal val entries = mutableStateListOf<Entry>()
    private var next = 0L

    internal var toast by mutableStateOf<ToastModel?>(null)

    val isShowing get() = entries.any { it.state.targetState }

    private fun push(kind: Kind, anchor: Rect? = null, content: @Composable (dismiss: () -> Unit) -> Unit) {
        entries.add(Entry(next++, kind, content).also { it.anchor = anchor })
    }

    fun dismissTop() {
        entries.lastOrNull { it.state.targetState }?.state?.targetState = false
    }

    fun dismissAll() = entries.forEach { it.state.targetState = false }

    /** A context menu near `anchor` (or centered without one). */
    fun menu(anchor: Anchor?, title: String? = null, entries: List<MenuEntry>) {
        push(Kind.Menu, anchor?.bounds) { dismiss -> MenuCard(title, entries, dismiss) }
    }

    /** A menu whose items load first (host catalogs over the relay). */
    fun menuAsync(anchor: Anchor?, title: String? = null, load: suspend () -> List<MenuEntry>) {
        push(Kind.Menu, anchor?.bounds) { dismiss ->
            var items by remember { mutableStateOf<List<MenuEntry>?>(null) }
            LaunchedEffect(Unit) { items = load() }
            val loaded = items
            if (loaded == null) {
                MenuCard(title, emptyList(), dismiss, loading = true)
            } else {
                MenuCard(title, loaded, dismiss)
            }
        }
    }

    fun sheet(content: @Composable (dismiss: () -> Unit) -> Unit) = push(Kind.Sheet, null, content)

    fun dialog(content: @Composable (dismiss: () -> Unit) -> Unit) = push(Kind.Dialog, null, content)

    /** Full-screen, edge to edge (the image viewer). */
    fun viewer(content: @Composable (dismiss: () -> Unit) -> Unit) = push(Kind.Viewer, null, content)

    fun confirm(title: String, message: String?, action: String, destructive: Boolean = false, onConfirm: () -> Unit) = dialog { dismiss ->
        DialogCard(title, message) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                CapsuleButton("Cancel", Modifier.weight(1f), style = ButtonStyle.Secondary, compact = true) { dismiss() }
                CapsuleButton(action, Modifier.weight(1f), style = if (destructive) ButtonStyle.Destructive else ButtonStyle.Primary, compact = true) {
                    dismiss()
                    onConfirm()
                }
            }
        }
    }

    fun alert(title: String, message: String?) = dialog { dismiss ->
        DialogCard(title, message) {
            CapsuleButton("OK", Modifier.fillMaxWidth(), style = ButtonStyle.Secondary, compact = true) { dismiss() }
        }
    }

    /** One text field (rename, new section). */
    fun prompt(title: String, initial: String = "", placeholder: String = "", action: String = "Save", onDone: (String) -> Unit) = dialog { dismiss ->
        var text by remember { mutableStateOf(initial) }
        val focus = remember { FocusRequester() }
        LaunchedEffect(Unit) { focus.requestFocus() }
        val c = LocalColors.current
        val submit = {
            val t = text.trim()
            if (t.isNotEmpty()) {
                dismiss()
                onDone(t)
            }
        }
        DialogCard(title, null) {
            Box(
                Modifier
                    .fillMaxWidth()
                    .background(c.controlFill, RoundedCornerShape(12.dp))
                    .padding(horizontal = 14.dp, vertical = 12.dp),
            ) {
                if (text.isEmpty()) ZText(placeholder, ZType.sans(16f), c.tertiary)
                BasicTextField(
                    text, { text = it },
                    Modifier
                        .fillMaxWidth()
                        .focusRequester(focus),
                    textStyle = ZType.sans(16f).copy(color = c.text),
                    cursorBrush = SolidColor(c.accent),
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { submit() }),
                )
            }
            Spacer(Modifier.height(16.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                CapsuleButton("Cancel", Modifier.weight(1f), style = ButtonStyle.Secondary, compact = true) { dismiss() }
                CapsuleButton(action, Modifier.weight(1f), enabled = text.isNotBlank(), compact = true) { submit() }
            }
        }
    }

    fun toast(text: String, action: String? = null, onAction: (() -> Unit)? = null) {
        toast = ToastModel(next++, text, action, onAction)
    }
}

internal data class ToastModel(val id: Long, val text: String, val action: String?, val onAction: (() -> Unit)?)

val LocalOverlays = staticCompositionLocalOf { Overlays() }

@Composable
fun OverlayHost(overlays: Overlays) {
    val c = LocalColors.current
    BackHandler(enabled = overlays.isShowing) { overlays.dismissTop() }
    // Drop entries whose exit animation finished.
    overlays.entries.filter { it.state.isIdle && !it.state.currentState && !it.state.targetState }
        .forEach { gone -> LaunchedEffect(gone.key) { overlays.entries.remove(gone) } }

    Box(Modifier.fillMaxSize()) {
        for (entry in overlays.entries) {
            val dismiss = { entry.state.targetState = false }
            val scrim = entry.kind != Overlays.Kind.Menu
            if (entry.kind != Overlays.Kind.Viewer) {
                AnimatedVisibility(entry.state, enter = fadeIn(tween(180)), exit = fadeOut(tween(160))) {
                    Box(
                        Modifier
                            .fillMaxSize()
                            .background(if (scrim) c.scrim else c.scrim.copy(alpha = 0.08f))
                            .clickable(remember { MutableInteractionSource() }, null) { dismiss() },
                    )
                }
            }
            when (entry.kind) {
                Overlays.Kind.Viewer -> AnimatedVisibility(
                    entry.state,
                    enter = fadeIn(tween(200)) + scaleIn(spring(dampingRatio = 0.9f, stiffness = 500f), initialScale = 0.94f),
                    exit = fadeOut(tween(160)),
                ) { entry.content(dismiss) }
                Overlays.Kind.Menu -> AnchoredMenu(entry, dismiss)
                Overlays.Kind.Sheet -> AnimatedVisibility(
                    entry.state,
                    enter = slideInVertically(spring(dampingRatio = 0.9f, stiffness = 500f)) { it },
                    exit = slideOutVertically(tween(200)) { it },
                ) {
                    SheetFrame(dismiss) { entry.content(dismiss) }
                }
                Overlays.Kind.Dialog -> AnimatedVisibility(
                    entry.state,
                    enter = fadeIn(tween(160)) + scaleIn(spring(dampingRatio = 0.8f, stiffness = 600f), initialScale = 0.92f),
                    exit = fadeOut(tween(120)) + scaleOut(targetScale = 0.96f),
                ) {
                    Box(
                        Modifier
                            .fillMaxSize()
                            .windowInsetsPadding(WindowInsets.systemBars)
                            .imePadding(),
                        contentAlignment = Alignment.Center,
                    ) { entry.content(dismiss) }
                }
            }
        }
        ToastHost(overlays)
    }
}

@Composable
private fun AnchoredMenu(entry: Overlays.Entry, dismiss: () -> Unit) {
    val density = LocalDensity.current
    BoxWithConstraints(
        Modifier
            .fillMaxSize()
            .windowInsetsPadding(WindowInsets.systemBars),
    ) {
        val maxW = constraints.maxWidth.toFloat()
        val maxH = constraints.maxHeight.toFloat()
        var cardH by remember { mutableStateOf(0f) }
        val margin = with(density) { 12.dp.toPx() }
        val width = with(density) { 264.dp.toPx() }
        val a = entry.anchor
        val top = with(density) { WindowInsets.systemBars.getTop(this).toFloat() }
        val (x, y, fromTop) = if (a == null) {
            Triple((maxW - width) / 2, (maxH - cardH) / 2, true)
        } else {
            val ax = (a.left).coerceIn(margin, maxOf(margin, maxW - width - margin))
            val below = a.bottom - top + with(density) { 6.dp.toPx() }
            val above = a.top - top - cardH - with(density) { 6.dp.toPx() }
            if (below + cardH <= maxH - margin || above < margin) Triple(ax, below.coerceAtMost(maxOf(margin, maxH - cardH - margin)), true)
            else Triple(ax, above, false)
        }
        AnimatedVisibility(
            entry.state,
            enter = fadeIn(tween(140)) + scaleIn(spring(dampingRatio = 0.8f, stiffness = 700f), initialScale = 0.9f, transformOrigin = androidx.compose.ui.graphics.TransformOrigin(0.1f, if (fromTop) 0f else 1f)),
            exit = fadeOut(tween(110)) + scaleOut(targetScale = 0.95f),
            modifier = Modifier
                .offset { IntOffset(x.roundToInt(), y.roundToInt()) }
                .onGloballyPositioned { cardH = it.size.height.toFloat() },
        ) {
            entry.content(dismiss)
        }
    }
}

@Composable
fun MenuCard(title: String?, entries: List<MenuEntry>, dismiss: () -> Unit, loading: Boolean = false) {
    val c = LocalColors.current
    val shape = RoundedCornerShape(18.dp)
    Column(
        Modifier
            .width(264.dp)
            .heightIn(max = 460.dp)
            .glass(c, shape, 16.dp)
            .verticalScroll(rememberScrollState())
            .padding(vertical = 6.dp),
    ) {
        if (title != null) {
            ZText(title, ZType.sans(12.5f, FontWeight.Medium), c.secondary, Modifier.padding(horizontal = 16.dp, vertical = 8.dp))
        }
        if (loading) {
            Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                StatusGlyph(GlyphKind.Spinner)
                Spacer(Modifier.width(10.dp))
                ZText("Loading…", ZType.sans(15f), c.secondary)
            }
        }
        if (!loading && entries.isEmpty()) {
            ZText("Nothing here", ZType.sans(15f), c.tertiary, Modifier.padding(16.dp))
        }
        for (e in entries) {
            when (e) {
                is MenuEntry.Header -> ZText(e.title, ZType.sans(12.5f, FontWeight.Medium), c.tertiary, Modifier.padding(start = 16.dp, end = 16.dp, top = 10.dp, bottom = 4.dp))
                MenuEntry.Divider -> Box(
                    Modifier
                        .padding(vertical = 5.dp)
                        .fillMaxWidth()
                        .height(0.75.dp)
                        .background(c.hairline),
                )
                is MenuEntry.Item -> Row(
                    Modifier
                        .fillMaxWidth()
                        .pressable(enabled = e.enabled, highlight = c.controlFill, shape = RoundedCornerShape(10.dp)) {
                            dismiss()
                            e.onClick()
                        }
                        .padding(horizontal = 16.dp, vertical = 11.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    val fg = when {
                        !e.enabled -> c.tertiary
                        e.destructive -> c.danger
                        else -> c.text
                    }
                    Column(Modifier.weight(1f)) {
                        ZText(e.title, ZType.sans(15.5f, if (e.checked) FontWeight.SemiBold else FontWeight.Normal), fg, maxLines = 2)
                        if (e.subtitle != null) ZText(e.subtitle, ZType.sans(12.5f), c.secondary, maxLines = 2)
                    }
                    Spacer(Modifier.width(10.dp))
                    when {
                        e.checked -> GlyphIcon(Glyph.Check, c.accent, size = 17.dp)
                        e.icon != null -> Box(Modifier.size(18.dp), contentAlignment = Alignment.Center) { e.icon.invoke() }
                        e.glyph != null -> GlyphIcon(e.glyph, fg, size = 18.dp)
                    }
                }
            }
        }
    }
}

@Composable
private fun SheetFrame(dismiss: () -> Unit, content: @Composable () -> Unit) {
    val c = LocalColors.current
    val density = LocalDensity.current
    var drag by remember { mutableStateOf(0f) }
    val scope = rememberCoroutineScope()
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        Column(
            Modifier
                .widthIn(max = 720.dp)
                .fillMaxWidth()
                .offset { IntOffset(0, drag.roundToInt()) }
                .windowInsetsPadding(WindowInsets.statusBars)
                .padding(top = 24.dp)
                .background(if (c.dark) c.elevated else c.background, RoundedCornerShape(topStart = 26.dp, topEnd = 26.dp))
                .clickable(remember { MutableInteractionSource() }, null) {}
                .windowInsetsPadding(WindowInsets.navigationBars)
                .imePadding(),
        ) {
            Box(
                Modifier
                    .fillMaxWidth()
                    .height(22.dp)
                    .pointerInput(Unit) {
                        detectVerticalDragGestures(
                            onDragEnd = {
                                if (drag > with(density) { 120.dp.toPx() }) dismiss()
                                scope.launch {
                                    while (drag > 0.5f) {
                                        drag *= 0.6f
                                        delay(16)
                                    }
                                    drag = 0f
                                }
                            },
                        ) { _, dy -> drag = (drag + dy).coerceAtLeast(0f) }
                    },
                contentAlignment = Alignment.Center,
            ) {
                Box(
                    Modifier
                        .size(width = 36.dp, height = 5.dp)
                        .background(c.tertiary.copy(alpha = 0.5f), CircleShape),
                )
            }
            content()
        }
    }
}

@Composable
fun DialogCard(title: String, message: String?, content: @Composable () -> Unit) {
    val c = LocalColors.current
    Column(
        Modifier
            .padding(28.dp)
            .widthIn(max = 380.dp)
            .fillMaxWidth()
            .glass(c, RoundedCornerShape(26.dp), 20.dp)
            .clickable(remember { MutableInteractionSource() }, null) {}
            .padding(20.dp),
    ) {
        ZText(title, ZType.sans(18f, FontWeight.SemiBold), c.text)
        if (message != null) {
            Spacer(Modifier.height(6.dp))
            ZText(message, ZType.sans(14.5f), c.secondary)
        }
        Spacer(Modifier.height(18.dp))
        content()
    }
}

@Composable
private fun ToastHost(overlays: Overlays) {
    val c = LocalColors.current
    val model = overlays.toast
    val state = remember { MutableTransitionState(false) }
    var shown by remember { mutableStateOf<ToastModel?>(null) }
    LaunchedEffect(model?.id) {
        if (model != null) {
            shown = model
            state.targetState = true
            delay(4000)
            state.targetState = false
            if (overlays.toast?.id == model.id) overlays.toast = null
        } else {
            state.targetState = false
        }
    }
    Box(
        Modifier
            .fillMaxSize()
            .windowInsetsPadding(WindowInsets.navigationBars)
            .windowInsetsPadding(WindowInsets.ime)
            .padding(bottom = 96.dp),
        contentAlignment = Alignment.BottomCenter,
    ) {
        AnimatedVisibility(
            state,
            enter = fadeIn() + slideInVertically(spring(dampingRatio = 0.85f)) { it / 2 },
            exit = fadeOut() + slideOutVertically { it / 3 },
        ) {
            val t = shown ?: return@AnimatedVisibility
            Row(
                Modifier
                    .padding(horizontal = 24.dp)
                    .glass(c, CircleShape, 12.dp)
                    .padding(horizontal = 20.dp, vertical = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                ZText(t.text, ZType.sans(15f, FontWeight.Medium), c.text, maxLines = 2, align = TextAlign.Start)
                if (t.action != null) {
                    Spacer(Modifier.width(14.dp))
                    ZText(
                        t.action, ZType.sans(15f, FontWeight.SemiBold), c.accent,
                        Modifier.pressable {
                            t.onAction?.invoke()
                            state.targetState = false
                            overlays.toast = null
                        },
                    )
                }
            }
        }
    }
}

/** Convenience: `Modifier.longPressMenu` shows a context menu anchored to the element. */
fun Modifier.menuAnchor(anchor: Anchor): Modifier = composed { this.anchor(anchor) }
