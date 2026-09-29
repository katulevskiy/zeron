package sh.zeron.android.design

import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.text.PlatformTextStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.LineHeightStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** Zeron's type: Geist for UI, Geist Mono for code, ids and numbers. */
object ZType {
    private val trim = LineHeightStyle(LineHeightStyle.Alignment.Center, LineHeightStyle.Trim.None)

    fun sans(size: Float, weight: FontWeight = FontWeight.Normal, lineHeight: Float? = null): TextStyle = TextStyle(
        fontFamily = Fonts.sans,
        fontWeight = weight,
        fontSize = size.sp,
        lineHeight = lineHeight?.sp ?: TextUnit.Unspecified,
        platformStyle = PlatformTextStyle(includeFontPadding = false),
        lineHeightStyle = trim,
    )

    fun mono(size: Float, weight: FontWeight = FontWeight.Normal): TextStyle = TextStyle(
        fontFamily = Fonts.mono,
        fontWeight = weight,
        fontSize = size.sp,
        platformStyle = PlatformTextStyle(includeFontPadding = false),
        lineHeightStyle = trim,
    )

    val Medium = FontWeight.Medium
    val Semibold = FontWeight.SemiBold
}

/**
 * Fixed-height rows scale with the user's font size, like Dynamic Type on
 * iOS: one clamped factor, so lists never self-size row by row.
 */
@Composable
fun typeFactor(): Float = LocalConfiguration.current.fontScale.coerceIn(0.85f, 1.6f)

@Composable
fun Dp.scaled(): Dp = (value * typeFactor()).dp

@Composable
fun ZText(
    text: String,
    style: TextStyle,
    color: Color,
    modifier: Modifier = Modifier,
    maxLines: Int = Int.MAX_VALUE,
    align: TextAlign? = null,
    overflow: TextOverflow = TextOverflow.Ellipsis,
) {
    BasicText(
        text = text,
        modifier = modifier,
        style = if (align != null) style.copy(color = color, textAlign = align) else style.copy(color = color),
        maxLines = maxLines,
        overflow = overflow,
        softWrap = maxLines != 1,
    )
}

/**
 * One line that fades out at its trailing edge instead of an ellipsis (the
 * desktop sidebar / iOS `FadingLabel`): the text itself fades, over any
 * background.
 */
@Composable
fun FadingText(
    text: String,
    style: TextStyle,
    color: Color,
    modifier: Modifier = Modifier,
    fade: Dp = 28.dp,
) {
    var overflowing by remember(text) { mutableStateOf(false) }
    BasicText(
        text = text,
        style = style.copy(color = color),
        maxLines = 1,
        softWrap = false,
        overflow = TextOverflow.Clip,
        onTextLayout = { overflowing = it.hasVisualOverflow },
        modifier = modifier
            .graphicsLayer { compositingStrategy = if (overflowing) CompositingStrategy.Offscreen else CompositingStrategy.Auto }
            .drawWithContent {
                drawContent()
                if (overflowing) {
                    val w = fade.toPx().coerceAtMost(size.width)
                    drawRect(
                        brush = Brush.horizontalGradient(
                            listOf(Color.Black, Color.Transparent),
                            startX = size.width - w,
                            endX = size.width,
                        ),
                        blendMode = BlendMode.DstIn,
                    )
                }
            },
    )
}

/**
 * Horizontal scroll content fades out at an edge that has more to show
 * (composer chips, like the iOS `FadingScrollView`), instead of a hard clip.
 */
fun Modifier.horizontalEdgeFade(scroll: androidx.compose.foundation.ScrollState, fade: Dp = 22.dp): Modifier = this
    .graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen }
    .drawWithContent {
        drawContent()
        val w = fade.toPx().coerceAtMost(size.width / 2)
        if (scroll.canScrollBackward) {
            drawRect(Brush.horizontalGradient(listOf(Color.Transparent, Color.Black), startX = 0f, endX = w), blendMode = BlendMode.DstIn)
        }
        if (scroll.canScrollForward) {
            drawRect(Brush.horizontalGradient(listOf(Color.Black, Color.Transparent), startX = size.width - w, endX = size.width), blendMode = BlendMode.DstIn)
        }
    }
