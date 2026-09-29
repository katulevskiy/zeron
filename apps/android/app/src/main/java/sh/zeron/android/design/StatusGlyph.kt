package sh.zeron.android.design

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlin.math.abs
import kotlin.math.floor

/**
 * The desktop's status glyphs:
 * - [Spinner]: the sidebar's mini spinner — a 2×3 grid of violet cells whose
 *   brightness chases around the ring (750 ms).
 * - [Trailer]: the transcript's 3×3 pastel gradient spinner.
 * - [Dot]: a static 7 dp circle (every non-working state).
 * - [Check]: completed-and-unseen.
 */
@Immutable
sealed interface GlyphKind {
    data object Spinner : GlyphKind
    data object Trailer : GlyphKind
    data class Dot(val color: Color) : GlyphKind
    data class Check(val color: Color) : GlyphKind
}

object GlyphMotion {
    const val PERIOD_MS = 750
    private val ring = arrayOf(intArrayOf(0, 1), intArrayOf(5, 2), intArrayOf(4, 3))

    /** `gspin_opacity`: hold bright, fall over 45 %, rest dim, snap back over the last 8 %. */
    fun opacity(t: Double, dim: Double = 0.1): Double {
        val u = t - floor(t)
        return when {
            u < 0.45 -> 1 + (dim - 1) * u / 0.45
            u < 0.92 -> dim
            else -> dim + (1 - dim) * (u - 0.92) / 0.08
        }
    }

    /** Phase of a spinner cell (row, col). */
    fun spinnerPhase(row: Int, col: Int) = ring[row][col] / 6.0

    fun trailerPhase(row: Int, col: Int) = (2 - row + abs(col - 1)) / 4.0
}

@Composable
fun StatusGlyph(kind: GlyphKind, modifier: Modifier = Modifier, size: Dp = 12.dp) {
    val colors = LocalColors.current
    val animated = kind is GlyphKind.Spinner || kind is GlyphKind.Trailer
    val t = if (animated) {
        val transition = rememberInfiniteTransition(label = "glyph")
        transition.animateFloat(
            0f, 1f,
            infiniteRepeatable(tween(GlyphMotion.PERIOD_MS, easing = LinearEasing), RepeatMode.Restart),
            label = "phase",
        )
    } else null
    Canvas(modifier.size(size)) {
        val phase = t?.value ?: 0f
        when (kind) {
            GlyphKind.Spinner -> drawSpinner(phase, colors.glyphTints)
            GlyphKind.Trailer -> drawTrailer(phase, colors.trailerTints)
            is GlyphKind.Dot -> drawCircle(kind.color, radius = 3.5.dp.toPx(), center = center)
            is GlyphKind.Check -> drawCheck(kind.color)
        }
    }
}

/** Mini spinner: 3.5 dp cells, 1.5 dp gap, 2 columns × 3 rows. */
fun DrawScope.drawSpinner(phase: Float, tints: List<Color>) {
    val d = 3.5.dp.toPx()
    val gap = 1.5.dp.toPx()
    val ox = center.x - (d * 2 + gap) / 2
    val oy = center.y - (d * 3 + gap * 2) / 2
    for (i in 0 until 6) {
        val row = i / 2
        val col = i % 2
        val a = GlyphMotion.opacity(phase - GlyphMotion.spinnerPhase(row, col)).toFloat()
        drawCircle(
            tints[row].copy(alpha = tints[row].alpha * a),
            radius = d / 2,
            center = Offset(ox + col * (d + gap) + d / 2, oy + row * (d + gap) + d / 2),
        )
    }
}

/** Transcript trailer: 3×3 pastel cells. */
fun DrawScope.drawTrailer(phase: Float, tints: List<Color>) {
    val d = 3.5.dp.toPx()
    val gap = 1.75.dp.toPx()
    val side = d * 3 + gap * 2
    for (i in 0 until 9) {
        val row = i / 3
        val col = i % 3
        val a = GlyphMotion.opacity(phase - GlyphMotion.trailerPhase(row, col)).toFloat()
        drawCircle(
            tints[row].copy(alpha = a),
            radius = d / 2,
            center = Offset(center.x - side / 2 + col * (d + gap) + d / 2, center.y - side / 2 + row * (d + gap) + d / 2),
        )
    }
}

/** check.svg: M3.5 8.5 l3 3 6-7 on a 16 grid. */
fun DrawScope.drawCheck(color: Color) {
    val s = size.minDimension / 16f * 1.05f
    val o = Offset(center.x - 8 * s, center.y - 8 * s)
    val p = Path().apply {
        moveTo(o.x + 3.5f * s, o.y + 8.5f * s)
        lineTo(o.x + 6.5f * s, o.y + 11.5f * s)
        lineTo(o.x + 12.5f * s, o.y + 4.5f * s)
    }
    drawPath(p, color, style = Stroke(width = 1.6.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round))
}
