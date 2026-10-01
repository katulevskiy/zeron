package sh.zeron.android.ui

import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * The effort slider's pure geometry and detent logic, kept apart from the
 * composable so it can be unit-tested: how a finger position becomes a step,
 * when a step *change* counts (hysteresis), where a fling lands and which
 * levels shimmer.
 */
object EffortScale {
    /**
     * Extra distance, in steps, a position must travel past the midpoint
     * between two stops before the slider moves to the next one. Jitter around
     * a boundary therefore doesn't re-fire a detent: coming back needs the same
     * margin on the other side.
     */
    const val HYSTERESIS = 0.12f

    /** How far ahead, in seconds, a release velocity is projected when picking the landing step. */
    const val FLING_HORIZON = 0.12f

    /** A step's place along the rail, 0 (first) … 1 (last). One step sits at 0. */
    fun fraction(step: Int, count: Int): Float =
        if (count <= 1) 0f else step.coerceIn(0, count - 1).toFloat() / (count - 1)

    /** The step whose stop is closest to [fraction]. */
    fun nearestStep(fraction: Float, count: Int): Int =
        if (count <= 1) 0 else (fraction.coerceIn(0f, 1f) * (count - 1)).roundToInt()

    /**
     * Position → step with hysteresis: [current] holds while the position stays
     * within half a step plus [HYSTERESIS] of its stop; past that, the nearest
     * stop wins.
     */
    fun snap(fraction: Float, count: Int, current: Int, hysteresis: Float = HYSTERESIS): Int {
        if (count <= 1) return 0
        val held = current.coerceIn(0, count - 1)
        val position = fraction.coerceIn(0f, 1f) * (count - 1)
        return if (abs(position - held) <= 0.5f + hysteresis) held else nearestStep(fraction, count)
    }

    /**
     * Pointer x → rail fraction. The thumb's centre travels between two insets
     * (half a thumb) so it never leaves the rail; the stops sit on that path.
     */
    fun fractionAt(x: Float, width: Float, inset: Float): Float {
        val travel = width - 2 * inset
        return if (travel <= 0f) 0f else ((x - inset) / travel).coerceIn(0f, 1f)
    }

    /** Where a release at [fraction] moving [velocity] (rail fractions per second) settles. */
    fun landing(fraction: Float, velocity: Float, count: Int, current: Int): Int =
        snap(fraction + velocity * FLING_HORIZON, count, current)

    /** Whether [step] is one of the rail's two ends (they get a firmer tick). */
    fun isEnd(step: Int, count: Int): Boolean = count > 1 && (step == 0 || step == count - 1)

    /**
     * How much a level shimmers, 0…1: only the top of the catalog's ladder
     * (`xhigh`, `max`, then the `ultra*` levels) glows, more the higher it is.
     */
    fun energy(level: String): Float = when (level.lowercase()) {
        "xhigh" -> 0.35f
        "max" -> 0.65f
        "ultra", "ultracode", "ultrathink" -> 1f
        else -> 0f
    }
}

/**
 * Tracks the step under a moving pointer and reports only *changes*: one
 * detent per crossing, never per frame, and not again on jitter at a boundary
 * (see [EffortScale.snap]).
 */
class DetentTracker(private val count: Int, start: Int) {
    var step: Int = start.coerceIn(0, (count - 1).coerceAtLeast(0))
        private set

    /** The new step when [fraction] moved onto another one, else null. */
    fun update(fraction: Float): Int? {
        val next = EffortScale.snap(fraction, count, step)
        if (next == step) return null
        step = next
        return next
    }

    /** Adopt a step set from outside (a keyboard, an accessibility action, a reset). */
    fun jump(to: Int) {
        step = to.coerceIn(0, (count - 1).coerceAtLeast(0))
    }
}
