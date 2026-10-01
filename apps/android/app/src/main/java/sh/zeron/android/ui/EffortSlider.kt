package sh.zeron.android.ui

import android.provider.Settings
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
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
import androidx.compose.runtime.Stable
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.graphicsLayer
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
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.feedback.Cue
import sh.zeron.android.feedback.Haptic
import sh.zeron.android.feedback.LocalFeedback
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.floor
import kotlin.math.roundToInt
import kotlin.math.sin

/** One stop on the slider: the wire id and the name shown. */
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
internal fun rememberPhase(periodMillis: Int): State<Float> {
    val transition = rememberInfiniteTransition(label = "phase")
    return transition.animateFloat(0f, 1f, infiniteRepeatable(tween(periodMillis, easing = LinearEasing)), label = "phase")
}

/**
 * Seconds since the slider appeared, and a shimmer phase that integrates the
 * level's shimmer speed (so a faster level speeds the sweep up without the band
 * jumping). Both are plain state read only by draw blocks, so a frame costs a
 * redraw of the small canvases and no recomposition.
 */
@Stable
private class FxClock {
    var time by mutableFloatStateOf(0f)
    var shimmer by mutableFloatStateOf(0f)
}

@Composable
private fun rememberFxClock(running: Boolean, level: State<Float>): FxClock {
    val clock = remember { FxClock() }
    LaunchedEffect(running) {
        if (!running) return@LaunchedEffect
        var last = 0L
        while (true) {
            androidx.compose.runtime.withFrameNanos { now ->
                val dt = if (last == 0L) 0f else ((now - last) / 1e9f).coerceAtMost(0.1f)
                last = now
                clock.time = (clock.time + dt) % 600f
                clock.shimmer = (clock.shimmer + dt * EffortFx.shimmerSpeed(level.value)) % 1000f
            }
        }
    }
    return clock
}

/**
 * The reasoning-effort slider: a pill rail with a dot at every level and a
 * springy pill thumb you drag, fling or tap, snapping to the levels. Every
 * level crossed fires one [Haptic.EffortStep] that is firmer the higher the
 * level, with a rising cue.
 *
 * The bar's colour and life follow the level: a calm cool fill and slow drift
 * at the bottom, warmer in the middle, and a hot gradient with a quick shimmer,
 * sparkles and a pulsing glow at the top. Dragging past either end stretches
 * the thumb with rubber-band resistance ([Haptic.Stretch], once) and on release
 * it bounces back ([Haptic.Rebound]). Settling on the top level plays a power
 * burst ([Haptic.Surge], [Cue.Surge]); settling on the lowest plays quick
 * streaks ([Haptic.Zip], [Cue.Zip]). [fast] adds speed streaks to the fill.
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
    val scope = rememberCoroutineScope()

    var dragging by remember { mutableStateOf(false) }
    var railWidth by remember { mutableFloatStateOf(0f) }
    var pointerFraction by remember { mutableFloatStateOf(EffortScale.fraction(step, count)) }
    val tracker = remember(count) { DetentTracker(count, step) }
    // A step set from outside (reset, accessibility, a new model) retargets the tracker.
    LaunchedEffect(step, count) { tracker.jump(step) }

    // Rubber band: how far (px, outward positive) the thumb is stretched past the end `stretchEnd` (+1 high, -1 low).
    var stretch by remember { mutableFloatStateOf(0f) }
    var stretchEnd by remember { mutableFloatStateOf(1f) }
    var stretchJob by remember { mutableStateOf<Job?>(null) }
    // Arrival bursts: 0 when idle, 0..1 while playing.
    var arrival by remember { mutableFloatStateOf(0f) }
    var arrivalEnd by remember { mutableStateOf(EffortEnd.High) }
    var arrivalJob by remember { mutableStateOf<Job?>(null) }
    val arrivals = remember(count) { ArrivalTracker().also { it.seed(step, count) } }
    var userMoved by remember { mutableStateOf(false) }
    var rebounded by remember { mutableStateOf(false) }

    /** One detent: report the step and feel it, firmer the higher it is. */
    fun detent(next: Int) {
        userMoved = true
        currentOnSelected(next)
        val f = currentFeedback
        f.haptic(Haptic.EffortStep, EffortScale.fraction(next, count))
        f.cue(Cue.Detent, next)
    }

    fun arrive(end: EffortEnd) {
        val f = currentFeedback
        if (end == EffortEnd.High) f.both(Haptic.Surge, Cue.Surge) else f.both(Haptic.Zip, Cue.Zip)
        if (reduceMotion) return
        arrivalJob?.cancel()
        arrivalEnd = end
        arrivalJob = scope.launch {
            animate(0f, 1f, animationSpec = tween(if (end == EffortEnd.High) EffortFx.BURST_MS else EffortFx.ZIP_MS, easing = LinearEasing)) { v, _ -> arrival = v }
            arrival = 0f
        }
    }

    // Arriving at an end plays once per visit: after a short dwell while the finger is still down, soon after release.
    LaunchedEffect(step, dragging, count) {
        if (arrivals.endOf(step, count) == null) {
            arrivals.settle(step, count, moved = false)
            userMoved = false
            return@LaunchedEffect
        }
        if (!userMoved) return@LaunchedEffect
        delay(if (dragging) ArrivalTracker.DWELL_MS else if (rebounded) 150L else 60L)
        arrivals.settle(step, count, moved = true)?.let { arrive(it) }
        rebounded = false
        userMoved = false
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
    val level = animateFloatAsState(EffortScale.fraction(step, count), tween(380), label = "level")
    val power by animateFloatAsState(if (fast) 1f else 0f, tween(420), label = "power")
    val clock = rememberFxClock(running = !reduceMotion, level = level)

    val scheme = MaterialTheme.colorScheme
    val dark = LocalDarkTheme.current
    val trackColor = scheme.onSurface.copy(alpha = if (dark) 0.10f else 0.075f)
    val outline = scheme.onSurface.copy(alpha = if (dark) 0.10f else 0.08f)
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
                val maxStretch = EffortFx.MAX_STRETCH_DP.dp.toPx()
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    down.consume()
                    stretchJob?.cancel()
                    val width = size.width.toFloat()
                    val travel = (width - 2 * inset).coerceAtLeast(1f)
                    val velocity = VelocityTracker()
                    var moved = false
                    // Stretch haptics fire once per pull: entering it, then reaching the wall.
                    var pulling = false
                    var atWall = false
                    fun follow(x: Float) {
                        val raw = EffortScale.rawFractionAt(x, width, inset)
                        val f = raw.coerceIn(0f, 1f)
                        pointerFraction = f
                        tracker.update(f)?.let { detent(it) }
                        val over = EffortFx.damp(EffortFx.overshoot(raw, travel), maxStretch)
                        if (over != 0f) stretchEnd = if (over > 0f) 1f else -1f
                        stretch = abs(over)
                        val mag = abs(over)
                        if (!pulling && mag > 1.5.dp.toPx()) {
                            pulling = true
                            currentFeedback.haptic(Haptic.Stretch, 0.35f)
                        } else if (pulling && mag < 0.5.dp.toPx()) {
                            pulling = false
                            atWall = false
                        }
                        if (pulling && !atWall && mag > 0.8f * maxStretch) {
                            atWall = true
                            currentFeedback.haptic(Haptic.Stretch, 1f)
                        }
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
                        val perSecond = velocity.calculateVelocity().x / travel
                        val land = EffortScale.landing(pointerFraction, perSecond, count, tracker.step)
                        if (land != tracker.step) {
                            tracker.jump(land)
                            detent(land)
                        }
                        val startStretch = stretch
                        if (startStretch > 1.dp.toPx()) {
                            // Let go while stretched: the thumb springs back with a bounce.
                            rebounded = true
                            currentFeedback.haptic(Haptic.Rebound)
                            currentFeedback.cue(Cue.Rebound)
                            if (reduceMotion) {
                                stretch = 0f
                            } else {
                                stretchJob = scope.launch {
                                    animate(startStretch, 0f, animationSpec = spring(dampingRatio = 0.26f, stiffness = 420f)) { v, _ -> stretch = v }
                                    stretch = 0f
                                }
                            }
                        } else {
                            stretch = 0f
                            if (moved && EffortScale.isEnd(tracker.step, count).not()) currentFeedback.haptic(Haptic.Select)
                        }
                    } finally {
                        dragging = false
                    }
                }
            },
    ) {
        // Under the rail: the pulsing glow of the very top level.
        Canvas(Modifier.fillMaxSize()) {
            val glow = EffortFx.glow(level.value)
            if (glow <= 0.001f) return@Canvas
            val pulse = if (reduceMotion) 0.7f else 0.35f + 0.65f * EffortFx.glowPulse(clock.time)
            val k = glow * pulse
            val color = Color(EffortFx.endColor(level.value))
            val railTop = (size.height - RailHeight.toPx()) / 2
            val corner = CornerRadius(RailHeight.toPx() / 2)
            val rail = Size(size.width, RailHeight.toPx())
            drawRoundRect(color.copy(alpha = 0.10f * k), Offset(-8.dp.toPx(), railTop - 8.dp.toPx()), Size(rail.width + 16.dp.toPx(), rail.height + 16.dp.toPx()), CornerRadius(corner.x + 8.dp.toPx()), style = Stroke(10.dp.toPx()))
            drawRoundRect(color.copy(alpha = 0.18f * k), Offset(-3.dp.toPx(), railTop - 3.dp.toPx()), Size(rail.width + 6.dp.toPx(), rail.height + 6.dp.toPx()), CornerRadius(corner.x + 3.dp.toPx()), style = Stroke(5.dp.toPx()))
        }
        Box(
            Modifier
                .align(Alignment.Center)
                .fillMaxWidth()
                .height(RailHeight)
                .clip(CircleShape)
                .background(trackColor)
                .border(1.dp, outline, CircleShape),
        ) {
            // The fill: colour from the level, up to the thumb.
            Canvas(Modifier.fillMaxSize()) {
                val inset = ThumbWidth.toPx() / 2
                val travel = size.width - 2 * inset
                val centre = inset + shown.value * travel
                val fillEnd = (centre + size.height / 2).coerceIn(0f, size.width)
                clipRect(left = 0f, top = 0f, right = fillEnd, bottom = size.height) {
                    drawRect(
                        Brush.horizontalGradient(
                            listOf(Color(EffortFx.startColor(level.value)).copy(alpha = 0.9f), Color(EffortFx.endColor(level.value))),
                            startX = 0f,
                            endX = fillEnd.coerceAtLeast(1f),
                        ),
                    )
                }
                for (i in 0 until count) {
                    val x = inset + EffortScale.fraction(i, count) * travel
                    val on = x < fillEnd - 2.dp.toPx()
                    drawCircle(
                        if (on) Color.White.copy(alpha = 0.7f) else scheme.onSurface.copy(alpha = 0.30f),
                        radius = 2.2.dp.toPx(),
                        center = Offset(x, size.height / 2),
                    )
                }
            }
            // The fill's life: shimmer, sparkles and (fast mode) streaks.
            Box(
                Modifier
                    .fillMaxSize()
                    .drawWithCache {
                        val band = (size.width * 0.34f).coerceAtLeast(60.dp.toPx())
                        val sweep = Brush.horizontalGradient(listOf(Color.Transparent, Color.White, Color.Transparent), startX = 0f, endX = band)
                        val streak = Path()
                        onDrawBehind {
                            val inset = ThumbWidth.toPx() / 2
                            val fillEnd = (inset + shown.value * (size.width - 2 * inset) + size.height / 2).coerceIn(0f, size.width)
                            if (fillEnd <= 0f) return@onDrawBehind
                            val lv = level.value
                            val phase = if (reduceMotion) 0.5f else clock.shimmer - floor(clock.shimmer)
                            clipRect(left = 0f, top = 0f, right = fillEnd, bottom = size.height) {
                                // Shimmer band sweeping across the filled part.
                                val x = -band + (fillEnd + band) * phase
                                translate(left = x) {
                                    drawRect(sweep, size = Size(band, size.height), alpha = EffortFx.shimmerAlpha(lv) + 0.08f * power)
                                }
                                if (power > 0.001f) drawStreaks(streak, phase, power, fillEnd)
                                if (!reduceMotion) drawSparkles(lv, clock.time, fillEnd)
                            }
                        }
                    },
            )
        }
        // Over the rail, under the thumb: the stretch bulge, fast mode's halo and the arrival bursts.
        Canvas(Modifier.fillMaxSize()) {
            val inset = ThumbWidth.toPx() / 2
            val travel = size.width - 2 * inset
            val centre = inset + shown.value * travel
            val cy = size.height / 2
            val lv = level.value
            if (stretch > 0.5f) {
                // The fill bulges past the rail's end, following the thumb.
                val h = RailHeight.toPx() * 0.92f
                val c = Color(EffortFx.endColor(lv))
                if (stretchEnd > 0f) {
                    drawRoundRect(c, Offset(size.width - h, cy - h / 2), Size(h + stretch, h), CornerRadius(h / 2))
                } else {
                    drawRoundRect(c, Offset(-stretch, cy - h / 2), Size(h + stretch, h), CornerRadius(h / 2))
                }
            }
            if (power > 0.001f) {
                val breath = 0.55f + 0.45f * (0.5f + 0.5f * sin((if (reduceMotion) 0.25f else clock.time * 0.45f) * 2f * PI.toFloat()))
                // Stacked discs rather than a gradient: nothing is allocated per frame.
                val halo = Color(EffortFx.endColor(lv))
                drawCircle(halo.copy(alpha = 0.10f * power * breath), radius = 38.dp.toPx(), center = Offset(centre, cy))
                drawCircle(halo.copy(alpha = 0.16f * power * breath), radius = 28.dp.toPx(), center = Offset(centre, cy))
                drawCircle(halo.copy(alpha = 0.22f * power * breath), radius = 20.dp.toPx(), center = Offset(centre, cy))
            }
            val t = arrival
            if (t > 0f && t < 1f) {
                if (arrivalEnd == EffortEnd.High) drawSurge(t, size.width, cy, inset) else drawZip(t, size.width, cy, inset)
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
                .graphicsLayer {
                    // Stretched: the outer edge follows the finger while the inner edge stays. The bounce
                    // back swings `stretch` below zero, which pushes the thumb in from its stop and squashes it.
                    val w = thumbWidth.toPx()
                    scaleX = (1f + stretch / w).coerceAtLeast(0.85f)
                    if (stretch < 0f) translationX = stretchEnd * stretch
                    scaleY = 1f - 0.08f * (stretch / EffortFx.MAX_STRETCH_DP.dp.toPx()).coerceIn(0f, 1f)
                    transformOrigin = TransformOrigin(if (stretchEnd > 0f) 0f else 1f, 0.5f)
                }
                .shadow(lift, CircleShape, ambientColor = Color.Black.copy(alpha = 0.5f), spotColor = Color.Black.copy(alpha = 0.5f))
                .background(thumbColor, CircleShape),
        )
    }
}

/** Slanted speed streaks racing toward the thumb (fast mode). */
private fun DrawScope.drawStreaks(streak: Path, phase: Float, power: Float, fillEnd: Float) {
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
        drawPath(streak, Color.White, alpha = 0.16f * power)
        sx += gap
    }
}

/** Twinkling glints scattered over the fill, more and livelier the higher the level. */
private fun DrawScope.drawSparkles(level: Float, time: Float, fillEnd: Float) {
    val n = EffortFx.sparkleCount(level)
    if (n == 0) return
    val rate = EffortFx.sparkleRate(level)
    val unit = 1.dp.toPx()
    for (i in 0 until n) {
        val life = EffortFx.sparkleLife(i, time * rate)
        if (life < 0.03f) continue
        val cycle = EffortFx.sparkleCycle(i, time * rate)
        val x = EffortFx.sparkleX(i, cycle) * fillEnd
        val y = EffortFx.sparkleY(i, cycle) * size.height
        val r = (1.4f + 1.4f * EffortFx.sparkleSize(i, cycle)) * unit * life
        val centre = Offset(x, y)
        drawCircle(Color.White.copy(alpha = 0.9f * life), radius = r * 0.55f, center = centre)
        val arm = r * 2.4f
        val c = Color.White.copy(alpha = 0.75f * life)
        drawLine(c, Offset(x - arm, y), Offset(x + arm, y), strokeWidth = unit * 0.9f, cap = StrokeCap.Round)
        drawLine(c, Offset(x, y - arm), Offset(x, y + arm), strokeWidth = unit * 0.9f, cap = StrokeCap.Round)
    }
}

/** The top level's power burst: a flare sweeping the bar, two energy rings and rays from the thumb. */
private fun DrawScope.drawSurge(t: Float, width: Float, cy: Float, inset: Float) {
    val unit = 1.dp.toPx()
    val hot = Color(EffortFx.endColor(1f))
    val core = Color(0xFFFFF1C7)
    val cx = width - inset
    // A bright flare racing from the left end to the thumb.
    val sweepT = (t / 0.55f).coerceIn(0f, 1f)
    val sweepX = (width - 2 * inset) * (1f - (1f - sweepT) * (1f - sweepT)) + inset
    val flareAlpha = if (t < 0.55f) 1f else EffortFx.fadeOut((t - 0.55f) / 0.45f)
    val bandH = RailHeight.toPx()
    drawRoundRect(core.copy(alpha = 0.35f * flareAlpha), Offset(sweepX - 26 * unit, cy - bandH / 2), Size(52 * unit, bandH), CornerRadius(bandH / 2))
    drawRoundRect(Color.White.copy(alpha = 0.8f * flareAlpha), Offset(sweepX - 5 * unit, cy - bandH / 2), Size(10 * unit, bandH), CornerRadius(5 * unit))
    // Energy rings from the thumb.
    val ring1 = t
    drawCircle(hot.copy(alpha = 0.85f * EffortFx.fadeOut(ring1)), radius = (8 + 74 * ring1) * unit, center = Offset(cx, cy), style = Stroke(((6f * (1f - ring1)) + 1f) * unit))
    if (t > 0.15f) {
        val ring2 = (t - 0.15f) / 0.85f
        drawCircle(core.copy(alpha = 0.7f * EffortFx.fadeOut(ring2)), radius = (6 + 52 * ring2) * unit, center = Offset(cx, cy), style = Stroke(((4f * (1f - ring2)) + 0.8f) * unit))
    }
    // Core flash.
    val flash = EffortFx.fadeOut(t * 1.4f)
    drawCircle(core.copy(alpha = 0.28f * flash), radius = 40 * unit, center = Offset(cx, cy))
    drawCircle(Color.White.copy(alpha = 0.45f * flash), radius = 20 * unit, center = Offset(cx, cy))
    // Rays fanning out the open side.
    val rayAlpha = EffortFx.fadeOut(t)
    for (k in 0..6) {
        val a = (PI / 2 + k * PI / 6).toFloat()
        val r0 = (16 + 40 * t) * unit
        val r1 = r0 + (10 + 12 * (1f - t)) * unit
        drawLine(
            hot.copy(alpha = 0.9f * rayAlpha),
            Offset(cx + cos(a) * r0, cy + sin(a) * r0),
            Offset(cx + cos(a) * r1, cy + sin(a) * r1),
            strokeWidth = 2 * unit,
            cap = StrokeCap.Round,
        )
    }
}

/** The lowest level's "faster and lighter": cool streaks racing right to left along the bar, then a pale wipe. */
private fun DrawScope.drawZip(t: Float, width: Float, cy: Float, inset: Float) {
    val unit = 1.dp.toPx()
    val left = inset - 14 * unit
    val right = width - 6 * unit
    val h = RailHeight.toPx()
    val cool = Color(0xFFBFF3FF)
    val ice = Color(0xFF6FD6E8)
    for (i in 0 until EffortFx.ZIP_STREAKS) {
        val p = EffortFx.zipProgress(i, t)
        val a = EffortFx.zipAlpha(p)
        if (a <= 0.01f) continue
        val lane = cy - h / 2 + h * (i + 0.5f) / EffortFx.ZIP_STREAKS
        val head = right - (right - left) * p
        val len = (34 + 38 * EffortFx.hash01(i * 5 + 1)) * unit
        // Brightest at the head, trailing off behind it.
        drawLine(ice.copy(alpha = 0.35f * a), Offset(head, lane), Offset(head + len, lane), strokeWidth = 2.4f * unit, cap = StrokeCap.Round)
        drawLine(cool.copy(alpha = 0.7f * a), Offset(head, lane), Offset(head + len * 0.55f, lane), strokeWidth = 1.6f * unit, cap = StrokeCap.Round)
        drawLine(Color.White.copy(alpha = 0.95f * a), Offset(head, lane), Offset(head + len * 0.22f, lane), strokeWidth = 1.2f * unit, cap = StrokeCap.Round)
    }
    // A pale wipe following the streaks.
    val wipe = ((t - 0.1f) / 0.7f).coerceIn(0f, 1f)
    if (wipe in 0.01f..0.99f) {
        val x = right - (right - left) * wipe
        drawRoundRect(cool.copy(alpha = 0.22f * (1f - wipe)), Offset(x - 20 * unit, cy - h / 2), Size(40 * unit, h), CornerRadius(h / 2))
    }
}
