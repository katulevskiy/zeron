package sh.zeron.android.ui

import android.provider.Settings
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.PointerId
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.setProgress
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.feedback.Cue
import sh.zeron.android.feedback.Haptic
import sh.zeron.android.feedback.LocalFeedback
import kotlin.math.abs
import kotlin.math.roundToInt
import kotlin.math.sin

/** One stop on the slider: the wire id (it decides the shimmer) and the name shown. */
data class EffortLevel(val id: String, val label: String)

private val SliderHeight = 52.dp
private val RailHeight = 34.dp
private val ThumbWidth = 56.dp
private val ThumbWidthPressed = 66.dp
private val ThumbHeight = 44.dp
private val ThumbHeightPressed = 46.dp

/** The user turned animations off system-wide (Developer options / Remove animations). */
@Composable
fun rememberReduceMotion(): Boolean {
    val resolver = LocalContext.current.contentResolver
    return remember { Settings.Global.getFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }
}

/** A looping 0…1 phase, only running while composed (callers gate it on there being something to animate). */
@Composable
private fun rememberPhase(periodMillis: Int): State<Float> {
    val transition = rememberInfiniteTransition(label = "phase")
    return transition.animateFloat(0f, 1f, infiniteRepeatable(tween(periodMillis, easing = LinearEasing)), label = "phase")
}

/**
 * The reasoning-effort slider: a pill rail with a dot at every level, an
 * accent fill up to a springy pill thumb you drag, fling or tap, snapping to
 * the levels. Each level crossed fires one detent through [LocalFeedback]
 * (a tick and a rising cue — firmer at the ends), and letting go after a
 * drag fires the "select" haptic. The top of the ladder shimmers; [fast]
 * brings the fill fully alive (sheen, speed streaks and a breathing halo).
 *
 * Accessible as a slider: its state reads "High, 3 of 5", TalkBack's set
 * progress and two custom actions step it, and arrow keys / Home / End
 * work on hardware keyboards.
 */
@Composable
fun EffortSlider(
    levels: List<EffortLevel>,
    selected: Int,
    onSelected: (Int) -> Unit,
    modifier: Modifier = Modifier,
    fast: Boolean = false,
    description: String = "Reasoning effort",
) {
    val count = levels.size
    if (count == 0) return
    val step = selected.coerceIn(0, count - 1)
    val feedback = LocalFeedback.current
    val reduceMotion = rememberReduceMotion()
    val currentOnSelected by rememberUpdatedState(onSelected)
    val currentFeedback by rememberUpdatedState(feedback)

    var dragging by remember { mutableStateOf(false) }
    var railWidth by remember { mutableFloatStateOf(0f) }
    var pointerFraction by remember { mutableFloatStateOf(EffortScale.fraction(step, count)) }
    val tracker = remember(count) { DetentTracker(count, step) }
    // A step set from outside (reset, accessibility, a new model) retargets the tracker.
    LaunchedEffect(step, count) { tracker.jump(step) }

    /** One detent: report the step and feel it. */
    fun detent(next: Int) {
        currentOnSelected(next)
        val f = currentFeedback
        if (EffortScale.isEnd(next, count)) f.both(Haptic.Threshold, Cue.Detent, next) else f.both(Haptic.Tick, Cue.Detent, next)
    }

    // The thumb tracks the finger tightly, then springs to its stop on release.
    val spatial = MaterialTheme.motionScheme.defaultSpatialSpec<Float>()
    val shown = animateFloatAsState(
        targetValue = if (dragging) pointerFraction else EffortScale.fraction(step, count),
        animationSpec = if (dragging) spring(dampingRatio = 1f, stiffness = 1600f) else spatial,
        label = "thumb",
    )
    val thumbWidth by animateDpAsState(if (dragging) ThumbWidthPressed else ThumbWidth, MaterialTheme.motionScheme.fastSpatialSpec(), label = "thumbWidth")
    val thumbHeight by animateDpAsState(if (dragging) ThumbHeightPressed else ThumbHeight, MaterialTheme.motionScheme.fastSpatialSpec(), label = "thumbHeight")
    val lift by animateDpAsState(if (dragging) 8.dp else 3.dp, MaterialTheme.motionScheme.fastEffectsSpec(), label = "lift")
    val energy by animateFloatAsState(EffortScale.energy(levels[step].id), tween(380), label = "energy")
    val power by animateFloatAsState(if (fast) 1f else 0f, tween(420), label = "power")
    val alive = !reduceMotion && (energy > 0.001f || power > 0.001f)
    val phase: State<Float> = if (alive) rememberPhase(2400) else remember { mutableFloatStateOf(0.5f) }

    val scheme = MaterialTheme.colorScheme
    val dark = LocalDarkTheme.current
    val trackColor = scheme.onSurface.copy(alpha = if (dark) 0.10f else 0.075f)
    val outline = scheme.onSurface.copy(alpha = if (dark) 0.10f else 0.08f)
    val accent = scheme.primary
    val thumbColor = if (dark) Color(0xFFF3F3F6) else Color.White

    Box(
        modifier
            .fillMaxWidth()
            .height(SliderHeight)
            .onSizeChanged { railWidth = it.width.toFloat() }
            .semantics(mergeDescendants = true) {
                contentDescription = description
                stateDescription = "${levels[step].label}, ${step + 1} of $count"
                progressBarRangeInfo = ProgressBarRangeInfo(step.toFloat(), 0f..(count - 1).toFloat(), steps = (count - 2).coerceAtLeast(0))
                setProgress { value ->
                    val to = value.roundToInt().coerceIn(0, count - 1)
                    if (to != step) detent(to)
                    true
                }
                customActions = listOf(
                    CustomAccessibilityAction("Increase effort") { if (step < count - 1) detent(step + 1); step < count - 1 },
                    CustomAccessibilityAction("Decrease effort") { if (step > 0) detent(step - 1); step > 0 },
                )
            }
            .focusable()
            .onKeyEvent { event ->
                if (event.type != KeyEventType.KeyDown) return@onKeyEvent false
                val to = when (event.key) {
                    Key.DirectionRight, Key.DirectionUp, Key.PageUp -> (step + 1).coerceAtMost(count - 1)
                    Key.DirectionLeft, Key.DirectionDown, Key.PageDown -> (step - 1).coerceAtLeast(0)
                    Key.MoveHome -> 0
                    Key.MoveEnd -> count - 1
                    else -> return@onKeyEvent false
                }
                if (to != step) detent(to)
                true
            }
            .pointerInput(count) {
                val inset = ThumbWidth.toPx() / 2
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    down.consume()
                    val width = size.width.toFloat()
                    val velocity = VelocityTracker()
                    var moved = false
                    fun follow(x: Float) {
                        val f = EffortScale.fractionAt(x, width, inset)
                        pointerFraction = f
                        tracker.update(f)?.let { detent(it) }
                    }
                    // Pressing is a tap already: the thumb heads for the stop under the finger.
                    pointerFraction = EffortScale.fractionAt(down.position.x, width, inset)
                    dragging = true
                    follow(down.position.x)
                    velocity.addPosition(down.uptimeMillis, down.position)
                    try {
                        var id: PointerId = down.id
                        while (true) {
                            val event = awaitPointerEvent()
                            val change = event.changes.firstOrNull { it.id == id } ?: break
                            if (change.positionChanged()) {
                                if (abs(change.position.x - down.position.x) > viewConfiguration.touchSlop) moved = true
                                follow(change.position.x)
                                velocity.addPosition(change.uptimeMillis, change.position)
                                change.consume()
                            }
                            if (!change.pressed) break
                        }
                        // Flung: carry on to the stop the release velocity projects to.
                        val perSecond = velocity.calculateVelocity().x / (width - 2 * inset).coerceAtLeast(1f)
                        val land = EffortScale.landing(pointerFraction, perSecond, count, tracker.step)
                        if (land != tracker.step) {
                            tracker.jump(land)
                            detent(land)
                        }
                        if (moved) currentFeedback.haptic(Haptic.Select)
                    } finally {
                        dragging = false
                    }
                }
            },
    ) {
        Box(
            Modifier
                .align(Alignment.Center)
                .fillMaxWidth()
                .height(RailHeight)
                .clip(CircleShape)
                .background(trackColor)
                .border(1.dp, outline, CircleShape),
        ) {
            Canvas(Modifier.fillMaxSize()) {
                val inset = ThumbWidth.toPx() / 2
                val travel = size.width - 2 * inset
                val centre = inset + shown.value * travel
                val fillEnd = (centre + size.height / 2).coerceIn(0f, size.width)
                clipRect(left = 0f, top = 0f, right = fillEnd, bottom = size.height) {
                    drawRect(Brush.horizontalGradient(listOf(accent.copy(alpha = 0.78f), accent), startX = 0f, endX = fillEnd.coerceAtLeast(1f)))
                    if (energy > 0.001f || power > 0.001f) drawLife(phase.value, energy, power, fillEnd)
                }
                for (i in 0 until count) {
                    val x = inset + EffortScale.fraction(i, count) * travel
                    val on = x < fillEnd - 2.dp.toPx()
                    drawCircle(
                        if (on) scheme.onPrimary.copy(alpha = 0.65f) else scheme.onSurface.copy(alpha = 0.30f),
                        radius = 2.2.dp.toPx(),
                        center = Offset(x, size.height / 2),
                    )
                }
            }
        }
        // Fast mode breathes a soft accent halo under the thumb.
        if (power > 0.001f) {
            Canvas(Modifier.fillMaxSize()) {
                val inset = ThumbWidth.toPx() / 2
                val centre = inset + shown.value * (size.width - 2 * inset)
                val breath = 0.55f + 0.45f * (0.5f + 0.5f * sin(phase.value * 2f * Math.PI.toFloat()))
                drawCircle(
                    Brush.radialGradient(
                        listOf(accent.copy(alpha = 0.55f * power * breath), Color.Transparent),
                        center = Offset(centre, size.height / 2),
                        radius = 38.dp.toPx(),
                    ),
                    radius = 38.dp.toPx(),
                    center = Offset(centre, size.height / 2),
                )
            }
        }
        Box(
            Modifier
                .offset {
                    val inset = ThumbWidth.toPx() / 2
                    IntOffset((inset + shown.value * (railWidth - 2 * inset) - thumbWidth.toPx() / 2).roundToInt(), 0)
                }
                .align(Alignment.CenterStart)
                .size(thumbWidth, thumbHeight)
                .shadow(lift, CircleShape, ambientColor = Color.Black.copy(alpha = 0.5f), spotColor = Color.Black.copy(alpha = 0.5f))
                .background(thumbColor, CircleShape),
        )
    }
}

/**
 * The fill's life: a soft band of light sweeping toward the thumb (stronger the
 * higher the level), and for fast mode slanted streaks racing the same way.
 */
private fun DrawScope.drawLife(phase: Float, energy: Float, power: Float, fillEnd: Float) {
    if (fillEnd <= 0f) return
    val band = (size.width * 0.38f).coerceAtLeast(60.dp.toPx())
    val strength = maxOf(energy, power * 0.8f)
    val x = -band + (size.width + 2 * band) * phase
    drawRect(
        Brush.horizontalGradient(
            listOf(Color.Transparent, Color.White.copy(alpha = 0.10f + 0.32f * strength), Color.Transparent),
            startX = x - band / 2,
            endX = x + band / 2,
        ),
    )
    if (power > 0.001f) {
        val streak = Path()
        val gap = 34.dp.toPx()
        val slant = size.height * 0.6f
        val width = 7.dp.toPx()
        val speed = ((phase * 3f) % 1f) * gap
        var sx = -gap + speed
        while (sx < fillEnd + gap) {
            streak.reset()
            streak.moveTo(sx, size.height)
            streak.lineTo(sx + width, size.height)
            streak.lineTo(sx + width + slant, 0f)
            streak.lineTo(sx + slant, 0f)
            streak.close()
            drawPath(streak, Color.White.copy(alpha = 0.16f * power))
            sx += gap
        }
    }
}
