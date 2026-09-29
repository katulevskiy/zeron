package sh.zeron.android.design

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.scale
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * Press feedback without Material ripples: the element dims (and optionally
 * a fill appears behind it) while the finger is down, like UIKit highlights.
 */
@OptIn(ExperimentalFoundationApi::class)
fun Modifier.pressable(
    enabled: Boolean = true,
    highlight: Color? = null,
    shape: Shape = RoundedCornerShape(16.dp),
    dim: Float = 0.55f,
    onLongClick: (() -> Unit)? = null,
    label: String? = null,
    onClick: () -> Unit,
): Modifier = composed {
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val haptics = LocalHapticFeedback.current
    val alpha by animateFloatAsState(if (pressed && highlight == null) dim else 1f, label = "press")
    val fill by animateColorAsState(if (pressed && highlight != null) highlight else Color.Transparent, label = "fill")
    this
        .graphicsLayer { this.alpha = alpha }
        .background(fill, shape)
        .combinedClickable(
            interactionSource = source,
            indication = null,
            enabled = enabled,
            role = Role.Button,
            onClickLabel = label,
            onLongClick = onLongClick?.let {
                {
                    haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                    it()
                }
            },
            onClick = onClick,
        )
}

/** Floating chrome surface (composer, panels, menus, toasts). */
fun Modifier.glass(colors: ZeronColors, shape: Shape, elevation: Dp = 10.dp): Modifier = this
    .shadow(elevation, shape, clip = false, ambientColor = Color.Black.copy(alpha = 0.25f), spotColor = Color.Black.copy(alpha = if (colors.dark) 0.5f else 0.18f))
    .clip(shape)
    .background(colors.glass, shape)
    .border(0.75.dp, colors.glassEdge, shape)

enum class ButtonStyle { Primary, Accent, Secondary, Ghost, Destructive }

@Composable
fun CapsuleButton(
    title: String,
    modifier: Modifier = Modifier,
    style: ButtonStyle = ButtonStyle.Primary,
    enabled: Boolean = true,
    busy: Boolean = false,
    glyph: Glyph? = null,
    compact: Boolean = false,
    onClick: () -> Unit,
) {
    val c = LocalColors.current
    val (bg, fg) = when (style) {
        ButtonStyle.Primary -> c.text to c.background
        ButtonStyle.Accent -> c.accent to Color.White
        ButtonStyle.Secondary -> c.controlFill to c.text
        ButtonStyle.Ghost -> Color.Transparent to c.secondary
        ButtonStyle.Destructive -> c.danger.copy(alpha = 0.12f) to c.danger
    }
    Row(
        modifier
            .defaultMinSize(minHeight = if (compact) 34.dp else 50.dp)
            .graphicsLayer { alpha = if (enabled) 1f else 0.4f }
            .clip(CircleShape)
            .background(bg)
            .pressable(enabled = enabled && !busy, shape = CircleShape, onClick = onClick)
            .padding(horizontal = if (compact) 14.dp else 20.dp, vertical = if (compact) 7.dp else 14.dp),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (busy) {
            StatusGlyph(GlyphKind.Spinner, size = 14.dp)
            Spacer(Modifier.width(8.dp))
        } else if (glyph != null) {
            GlyphIcon(glyph, fg, size = if (compact) 15.dp else 18.dp)
            Spacer(Modifier.width(7.dp))
        }
        ZText(title, ZType.sans(if (compact) 14f else 16.5f, FontWeight.SemiBold), fg, maxLines = 1)
    }
}

/** Round icon button on a glass plate (toolbar …, back, jump-to-latest). */
@Composable
fun CircleIconButton(
    glyph: Glyph,
    modifier: Modifier = Modifier,
    size: Dp = 40.dp,
    iconSize: Dp = 18.dp,
    tint: Color? = null,
    plate: Boolean = true,
    label: String? = null,
    onClick: () -> Unit,
) {
    val c = LocalColors.current
    Box(
        modifier
            .size(size)
            .then(if (plate) Modifier.glass(c, CircleShape, 6.dp) else Modifier)
            .pressable(shape = CircleShape, label = label, onClick = onClick)
            .semantics { if (label != null) contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        GlyphIcon(glyph, tint ?: c.text, size = iconSize)
    }
}

/** A composer/context chip: [icon] title, capsule, control fill (tinted chips wash their tone). */
@Composable
fun Chip(
    title: String,
    modifier: Modifier = Modifier,
    tint: Color? = null,
    mono: Boolean = false,
    leading: (@Composable () -> Unit)? = null,
    onClick: (() -> Unit)? = null,
) {
    val c = LocalColors.current
    Row(
        modifier
            .height(34.dp)
            .clip(CircleShape)
            .background(tint?.copy(alpha = 0.1f) ?: c.controlFill)
            .then(if (onClick != null) Modifier.pressable(shape = CircleShape, onClick = onClick) else Modifier)
            .padding(horizontal = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (leading != null) {
            leading()
            Spacer(Modifier.width(6.dp))
        }
        ZText(
            title,
            if (mono) ZType.mono(12.5f, FontWeight.Medium) else ZType.sans(13.5f, FontWeight.Medium),
            tint?.copy(alpha = 0.85f) ?: c.text,
            maxLines = 1,
        )
    }
}

/** Screen header: back button, centered two-line title, trailing actions. */
@Composable
fun TopBar(
    title: String,
    subtitle: String? = null,
    onBack: (() -> Unit)? = null,
    modifier: Modifier = Modifier,
    background: Color? = null,
    actions: @Composable RowScope.() -> Unit = {},
) {
    val c = LocalColors.current
    Box(
        modifier
            .fillMaxWidth()
            .then(if (background != null) Modifier.background(background) else Modifier)
            .windowInsetsPadding(WindowInsets.statusBars)
            .height(56.dp)
            .padding(horizontal = 10.dp),
    ) {
        if (onBack != null) {
            CircleIconButton(Glyph.ChevronLeft, Modifier.align(Alignment.CenterStart), label = "Back", onClick = onBack)
        }
        Column(
            Modifier
                .align(Alignment.Center)
                .padding(horizontal = 64.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            FadingText(title, ZType.sans(16f, FontWeight.SemiBold), c.text)
            if (!subtitle.isNullOrEmpty()) {
                FadingText(subtitle, ZType.sans(12f), c.secondary)
            }
        }
        Row(Modifier.align(Alignment.CenterEnd), horizontalArrangement = Arrangement.spacedBy(8.dp), content = actions)
    }
}

/** Tab-root header with a large title (Sessions / Projects / Settings). */
@Composable
fun LargeTitle(title: String, modifier: Modifier = Modifier, actions: @Composable RowScope.() -> Unit = {}) {
    val c = LocalColors.current
    Row(
        modifier
            .fillMaxWidth()
            .padding(start = 20.dp, end = 12.dp, top = 8.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ZText(title, ZType.sans(30f, FontWeight.SemiBold), c.text, Modifier.weight(1f), maxLines = 1)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), content = actions)
    }
}

/** An iOS-style switch in Zeron's accent. */
@Composable
fun Toggle(on: Boolean, modifier: Modifier = Modifier, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    val c = LocalColors.current
    val x by animateFloatAsState(if (on) 1f else 0f, spring(dampingRatio = 0.8f, stiffness = 700f), label = "toggle")
    val track by animateColorAsState(if (on) c.accent else c.controlFill.copy(alpha = 0.16f), label = "track")
    Box(
        modifier
            .size(width = 50.dp, height = 30.dp)
            .graphicsLayer { alpha = if (enabled) 1f else 0.4f }
            .clip(CircleShape)
            .background(track)
            .pressable(enabled = enabled, shape = CircleShape) { onChange(!on) },
    ) {
        Box(
            Modifier
                .padding(2.dp)
                .offset(x = (20 * x).dp)
                .size(26.dp)
                .shadow(2.dp, CircleShape)
                .background(Color.White, CircleShape),
        )
    }
}

/** A settings-style row inside a grouped card. */
@Composable
fun ListRow(
    title: String,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    glyph: Glyph? = null,
    icon: (@Composable () -> Unit)? = null,
    tint: Color? = null,
    destructive: Boolean = false,
    chevron: Boolean = false,
    trailing: (@Composable () -> Unit)? = null,
    onClick: (() -> Unit)? = null,
) {
    val c = LocalColors.current
    Row(
        modifier
            .fillMaxWidth()
            .defaultMinSize(minHeight = 52.dp)
            .then(if (onClick != null) Modifier.pressable(highlight = c.controlFill, shape = RoundedCornerShape(0.dp), onClick = onClick) else Modifier)
            .padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        when {
            icon != null -> {
                Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) { icon() }
                Spacer(Modifier.width(14.dp))
            }
            glyph != null -> {
                GlyphIcon(glyph, if (destructive) c.danger else tint ?: c.secondary, size = 21.dp)
                Spacer(Modifier.width(14.dp))
            }
        }
        Column(Modifier.weight(1f)) {
            ZText(title, ZType.sans(16f, FontWeight.Medium), if (destructive) c.danger else c.text, maxLines = 2)
            if (!subtitle.isNullOrEmpty()) {
                ZText(subtitle, ZType.sans(13f), c.secondary, Modifier.padding(top = 2.dp), maxLines = 4)
            }
        }
        if (trailing != null) {
            Spacer(Modifier.width(10.dp))
            trailing()
        }
        if (chevron) {
            Spacer(Modifier.width(8.dp))
            GlyphIcon(Glyph.ChevronRight, c.tertiary, size = 14.dp)
        }
    }
}

/** Grouped card with an optional small caps header (settings sections). */
@Composable
fun Group(header: String? = null, footer: String? = null, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    val c = LocalColors.current
    Column(modifier.padding(horizontal = 16.dp)) {
        if (header != null) {
            ZText(header.uppercase(), ZType.sans(12.5f, FontWeight.Medium).copy(letterSpacing = 0.6.sp), c.secondary, Modifier.padding(start = 16.dp, top = 22.dp, bottom = 8.dp))
        }
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(18.dp))
                .background(c.elevated),
        ) { content() }
        if (footer != null) {
            ZText(footer, ZType.sans(12.5f), c.tertiary, Modifier.padding(start = 16.dp, end = 16.dp, top = 8.dp))
        }
    }
}

@Composable
fun Hairline(modifier: Modifier = Modifier, inset: Dp = 54.dp) {
    val c = LocalColors.current
    Box(
        modifier
            .fillMaxWidth()
            .padding(start = inset)
            .height(0.75.dp)
            .background(c.hairline),
    )
}

/**
 * The desktop's project monogram: a rounded tile in the project's tone
 * (fill @ 0.08, letter @ 0.85, mono medium).
 */
@Composable
fun ProjectTile(name: String, colorIndex: Int, size: Dp = 14.dp, modifier: Modifier = Modifier) {
    val tone = LocalColors.current.project(colorIndex)
    val k = size.value / 13f
    Box(
        modifier
            .size(size)
            .background(tone.copy(alpha = 0.08f), RoundedCornerShape((3 * k).dp)),
        contentAlignment = Alignment.Center,
    ) {
        ZText(
            name.trim().firstOrNull()?.uppercase() ?: "?",
            ZType.mono(9f * k, FontWeight.Medium).copy(fontSize = (9f * k).sp),
            tone.copy(alpha = 0.85f),
            align = TextAlign.Center,
        )
    }
}

enum class PrState { Open, Merged, Closed }

/** The desktop's pull-request badge: PR icon + number in mono, tinted by state. */
@Composable
fun PrBadge(number: ULong, state: PrState, modifier: Modifier = Modifier, fontSize: Float = 11f) {
    val c = LocalColors.current
    val tone = when (state) {
        PrState.Open -> c.success
        PrState.Merged -> c.accent
        PrState.Closed -> c.danger
    }
    Row(
        modifier
            .height((fontSize * 1.65f).dp)
            .background(tone.copy(alpha = 0.08f), RoundedCornerShape((fontSize * 0.45f).dp))
            .padding(horizontal = (fontSize * 0.45f).dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PrIcon(tone.copy(alpha = 0.85f), fontSize.dp)
        Spacer(Modifier.width(3.dp))
        ZText("$number", ZType.mono(fontSize, FontWeight.Medium), tone.copy(alpha = 0.85f))
    }
}

/** `pull-request.svg` (24 grid, 1.5 stroke). */
@Composable
fun PrIcon(tint: Color, size: Dp) {
    Box(
        Modifier
            .size(size)
            .drawBehind {
                val s = this.size.minDimension / 24f
                val stroke = androidx.compose.ui.graphics.drawscope.Stroke(
                    width = maxOf(1f, 1.5f * s * 1.15f),
                    cap = androidx.compose.ui.graphics.StrokeCap.Round,
                    join = androidx.compose.ui.graphics.StrokeJoin.Round,
                )
                for ((cx, cy) in listOf(6f to 5f, 6f to 19f, 18f to 19f)) {
                    drawCircle(tint, 2.25f * s, androidx.compose.ui.geometry.Offset(cx * s, cy * s), style = stroke)
                }
                val p = androidx.compose.ui.graphics.Path().apply {
                    moveTo(6 * s, 7.25f * s); lineTo(6 * s, 16.75f * s)
                    moveTo(15 * s, 5 * s); lineTo(15.75f * s, 5 * s)
                    quadraticTo(18 * s, 5 * s, 18 * s, 7.25f * s)
                    lineTo(18 * s, 16.75f * s)
                    moveTo(12.75f * s, 7.75f * s); lineTo(15.25f * s, 5 * s); lineTo(12.75f * s, 2.25f * s)
                }
                drawPath(p, tint, style = stroke)
            },
    )
}

/** Section header text used above lists. */
@Composable
fun SectionLabel(text: String, modifier: Modifier = Modifier) {
    val c = LocalColors.current
    ZText(text, ZType.sans(13f, FontWeight.SemiBold), c.secondary, modifier.padding(start = 20.dp, top = 18.dp, bottom = 6.dp))
}

/** Empty-state block: glyph, title, detail. */
@Composable
fun EmptyState(glyph: Glyph, title: String, detail: String?, modifier: Modifier = Modifier, action: (@Composable () -> Unit)? = null) {
    val c = LocalColors.current
    Column(modifier.padding(32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        GlyphIcon(glyph, c.tertiary, size = 34.dp)
        Spacer(Modifier.height(14.dp))
        ZText(title, ZType.sans(17f, FontWeight.SemiBold), c.text, align = TextAlign.Center)
        if (detail != null) {
            Spacer(Modifier.height(6.dp))
            ZText(detail, ZType.sans(14f), c.secondary, align = TextAlign.Center)
        }
        if (action != null) {
            Spacer(Modifier.height(18.dp))
            action()
        }
    }
}
