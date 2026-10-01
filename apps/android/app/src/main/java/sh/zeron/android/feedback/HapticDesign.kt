package sh.zeron.android.feedback

import android.os.Build
import android.os.VibrationEffect
import android.os.VibrationEffect.Composition as C
import android.view.HapticFeedbackConstants as H

/** One primitive in a composition: [scale] 0..1 is the Standard strength, [delayMs] precedes it. */
data class Step(val primitive: Int, val scale: Float, val delayMs: Int = 0)

/** One segment of the waveform fallback: vibrate at [amplitude] (0 = pause) for [ms]. */
data class Segment(val ms: Long, val amplitude: Int)

/**
 * How a [Haptic] is felt. The engine walks a ladder from the richest effect
 * the device can render down to a plain pulse:
 *
 * 1. The platform's own `performHapticFeedback` constant (OEM tuned, so it
 *    feels native) when [preferView] and the strength is Standard.
 * 2. A `VibrationEffect.Composition` of primitives, scaled by strength.
 * 3. A view constant for patterns that are not [preferView] (Standard only).
 * 4. The designed [waveform] (amplitude control), then the predefined effect,
 *    then the waveform without amplitudes.
 */
class HapticSpec(
    val haptic: Haptic,
    val preferView: Boolean,
    /** SDK level -> `HapticFeedbackConstants` value, or null when this device has none for the moment. */
    val view: (Int) -> Int?,
    /** Alternatives tried in order; the first whose primitives are all supported wins. */
    val compositions: List<List<Step>>,
    val predefined: Int,
    val waveform: List<Segment>,
    /** Prefer [waveform] over [predefined] when amplitude control exists (the compound patterns). */
    val designedWaveform: Boolean = false,
    /** 0 light (scrolling-class), 1 action, 2 event, 3 alert. */
    val priority: Int,
    /** The same haptic never repeats inside this many ms. */
    val minGapMs: Long,
)

object HapticTable {
    private fun api(level: Int, constant: Int): (Int) -> Int? = { sdk -> if (sdk >= level) constant else null }
    private fun api(level: Int, constant: Int, below: Int): (Int) -> Int? = { sdk -> if (sdk >= level) constant else below }

    fun spec(haptic: Haptic): HapticSpec = when (haptic) {
        // The faintest detent: slider steps and discrete pickers. API 34 has a dedicated frequent tick.
        Haptic.Tick -> HapticSpec(
            haptic, true, api(34, H.SEGMENT_FREQUENT_TICK, H.CLOCK_TICK),
            listOf(listOf(Step(C.PRIMITIVE_LOW_TICK, 0.5f)), listOf(Step(C.PRIMITIVE_TICK, 0.3f))),
            VibrationEffect.EFFECT_TICK, listOf(Segment(10, 40)), priority = 0, minGapMs = 70,
        )
        // A choice was made: crisper than Tick.
        Haptic.Select -> HapticSpec(
            haptic, true, api(34, H.SEGMENT_TICK, H.CONTEXT_CLICK),
            listOf(listOf(Step(C.PRIMITIVE_TICK, 0.7f)), listOf(Step(C.PRIMITIVE_CLICK, 0.35f))),
            VibrationEffect.EFFECT_TICK, listOf(Segment(12, 90)), priority = 0, minGapMs = 50,
        )
        // Switches: a rising pair for on, a falling pair for off (the system toggle constants from API 34).
        Haptic.ToggleOn -> HapticSpec(
            haptic, true, api(34, H.TOGGLE_ON),
            listOf(
                listOf(Step(C.PRIMITIVE_LOW_TICK, 0.5f), Step(C.PRIMITIVE_TICK, 0.8f, 20)),
                listOf(Step(C.PRIMITIVE_TICK, 0.5f), Step(C.PRIMITIVE_CLICK, 0.6f, 25)),
            ),
            VibrationEffect.EFFECT_CLICK, listOf(Segment(8, 60), Segment(20, 0), Segment(12, 130)), priority = 1, minGapMs = 80,
        )
        Haptic.ToggleOff -> HapticSpec(
            haptic, true, api(34, H.TOGGLE_OFF),
            listOf(
                listOf(Step(C.PRIMITIVE_TICK, 0.7f), Step(C.PRIMITIVE_LOW_TICK, 0.4f, 20)),
                listOf(Step(C.PRIMITIVE_CLICK, 0.5f), Step(C.PRIMITIVE_TICK, 0.4f, 25)),
            ),
            VibrationEffect.EFFECT_TICK, listOf(Segment(12, 110), Segment(20, 0), Segment(8, 50)), priority = 1, minGapMs = 80,
        )
        // A press that begins something.
        Haptic.Press -> HapticSpec(
            haptic, true, api(30, H.GESTURE_START),
            listOf(listOf(Step(C.PRIMITIVE_CLICK, 0.4f)), listOf(Step(C.PRIMITIVE_TICK, 0.8f))),
            VibrationEffect.EFFECT_TICK, listOf(Segment(14, 100)), priority = 1, minGapMs = 120,
        )
        // A committed action: send, create, save, start.
        Haptic.Confirm -> HapticSpec(
            haptic, true, api(30, H.CONFIRM),
            listOf(listOf(Step(C.PRIMITIVE_CLICK, 0.65f)), listOf(Step(C.PRIMITIVE_TICK, 0.9f))),
            VibrationEffect.EFFECT_CLICK, listOf(Segment(20, 170)), priority = 1, minGapMs = 120,
        )
        // Soft rise, then a small settle: a turn completed, a transfer arrived.
        Haptic.Success -> HapticSpec(
            haptic, false, { null },
            listOf(
                listOf(Step(C.PRIMITIVE_QUICK_RISE, 0.45f), Step(C.PRIMITIVE_LOW_TICK, 0.55f, 40)),
                listOf(Step(C.PRIMITIVE_CLICK, 0.5f), Step(C.PRIMITIVE_TICK, 0.5f, 60)),
            ),
            VibrationEffect.EFFECT_CLICK, listOf(Segment(24, 70), Segment(24, 130), Segment(30, 0), Segment(18, 110)),
            designedWaveform = true, priority = 2, minGapMs = 400,
        )
        // Two crisp taps: something needs you.
        Haptic.Attention -> HapticSpec(
            haptic, false, { null },
            listOf(
                listOf(Step(C.PRIMITIVE_TICK, 0.85f), Step(C.PRIMITIVE_TICK, 0.85f, 90)),
                listOf(Step(C.PRIMITIVE_CLICK, 0.6f), Step(C.PRIMITIVE_CLICK, 0.6f, 90)),
            ),
            VibrationEffect.EFFECT_DOUBLE_CLICK, listOf(Segment(18, 200), Segment(70, 0), Segment(18, 200)),
            designedWaveform = true, priority = 2, minGapMs = 400,
        )
        // A short, heavy double: failed or refused.
        Haptic.Error -> HapticSpec(
            haptic, false, api(30, H.REJECT),
            listOf(
                listOf(Step(C.PRIMITIVE_CLICK, 1f), Step(C.PRIMITIVE_THUD, 0.8f, 60)),
                listOf(Step(C.PRIMITIVE_CLICK, 1f), Step(C.PRIMITIVE_CLICK, 0.9f, 70)),
            ),
            VibrationEffect.EFFECT_HEAVY_CLICK, listOf(Segment(35, 255), Segment(55, 0), Segment(45, 230)),
            designedWaveform = true, priority = 3, minGapMs = 400,
        )
        Haptic.LongPress -> HapticSpec(
            haptic, true, api(0, H.LONG_PRESS),
            listOf(listOf(Step(C.PRIMITIVE_CLICK, 0.7f)), listOf(Step(C.PRIMITIVE_TICK, 1f))),
            VibrationEffect.EFFECT_CLICK, listOf(Segment(25, 160)), priority = 1, minGapMs = 300,
        )
        // A swipe / drag crossed the point of commitment.
        Haptic.Threshold -> HapticSpec(
            haptic, true, api(34, H.GESTURE_THRESHOLD_ACTIVATE),
            listOf(listOf(Step(C.PRIMITIVE_CLICK, 0.55f)), listOf(Step(C.PRIMITIVE_TICK, 0.9f))),
            VibrationEffect.EFFECT_CLICK, listOf(Segment(16, 150)), priority = 1, minGapMs = 150,
        )
        // Delete, uninstall, discard: a low thud with a click on top.
        Haptic.Heavy -> HapticSpec(
            haptic, false, { null },
            listOf(
                listOf(Step(C.PRIMITIVE_THUD, 0.85f), Step(C.PRIMITIVE_CLICK, 0.5f, 30)),
                listOf(Step(C.PRIMITIVE_CLICK, 1f)),
            ),
            VibrationEffect.EFFECT_HEAVY_CLICK, listOf(Segment(40, 255)), priority = 2, minGapMs = 300,
        )
        // Starred, pinned: a small pop that decays.
        Haptic.Pop -> HapticSpec(
            haptic, false, { null },
            listOf(
                listOf(Step(C.PRIMITIVE_TICK, 0.8f), Step(C.PRIMITIVE_LOW_TICK, 0.35f, 25)),
                listOf(Step(C.PRIMITIVE_TICK, 0.8f)),
            ),
            VibrationEffect.EFFECT_TICK, listOf(Segment(12, 120), Segment(14, 0), Segment(8, 50)), priority = 1, minGapMs = 120,
        )
    }

    val all: List<HapticSpec> get() = Haptic.entries.map(::spec)
}

/** What the device can do, probed once at start. */
data class HapticCapabilities(
    val sdk: Int,
    /** Primitive ids this vibrator renders (`areAllPrimitivesSupported`). */
    val primitives: Set<Int>,
    val amplitudeControl: Boolean,
    /** A window view is attached for `performHapticFeedback`. */
    val hasView: Boolean,
)

/** The effect chosen for one haptic on one device. */
sealed interface HapticPlan {
    data class ViewConstant(val constant: Int) : HapticPlan
    data class Composition(val steps: List<Step>) : HapticPlan
    data class Predefined(val effect: Int) : HapticPlan
    data class Waveform(val timings: LongArray, val amplitudes: IntArray?) : HapticPlan {
        override fun equals(other: Any?) = other is Waveform && timings.contentEquals(other.timings) && amplitudes.contentEquals(other.amplitudes)
        override fun hashCode() = timings.contentHashCode() * 31 + amplitudes.contentHashCode()
    }

    /** Nothing to play, with why (kept for the debug log). */
    data class Skip(val reason: String) : HapticPlan
}

object HapticPlanner {
    /** Primitive scale at [strength], never inaudible and never above full. */
    fun scaled(scale: Float, strength: HapticStrength): Float = (scale * strength.scale).coerceIn(0.05f, 1f)

    fun plan(haptic: Haptic, strength: HapticStrength, caps: HapticCapabilities): HapticPlan {
        val spec = HapticTable.spec(haptic)
        val standard = strength == HapticStrength.Standard
        val view = if (caps.hasView && standard) spec.view(caps.sdk) else null

        if (spec.preferView && view != null) return HapticPlan.ViewConstant(view)

        if (caps.sdk >= Build.VERSION_CODES.R) {
            spec.compositions.firstOrNull { steps -> steps.all { it.primitive in caps.primitives } }?.let { steps ->
                return HapticPlan.Composition(steps.map { it.copy(scale = scaled(it.scale, strength)) })
            }
        }
        if (view != null) return HapticPlan.ViewConstant(view)

        // Hardware without primitives. Without amplitude control a Subtle request can only be honoured
        // by dropping the lightest haptics: playing them at full strength would be the opposite.
        if (!caps.amplitudeControl && strength == HapticStrength.Subtle && spec.priority == 0) {
            return HapticPlan.Skip("subtle: no amplitude control")
        }
        if (caps.amplitudeControl && (spec.designedWaveform || !standard)) return waveform(spec, strength, true)
        if (standard || !caps.amplitudeControl) return HapticPlan.Predefined(spec.predefined)
        return waveform(spec, strength, caps.amplitudeControl)
    }

    fun waveform(spec: HapticSpec, strength: HapticStrength, amplitudes: Boolean): HapticPlan.Waveform {
        if (!amplitudes) return HapticPlan.Waveform(onOffTimings(spec.waveform), null)
        val timings = LongArray(spec.waveform.size) { spec.waveform[it].ms }
        val amps = IntArray(spec.waveform.size) {
            val a = spec.waveform[it].amplitude
            if (a == 0) 0 else (a * strength.scale).toInt().coerceIn(1, 255)
        }
        return HapticPlan.Waveform(timings, amps)
    }

    /** `createWaveform(timings, repeat)` alternates off, on, off, ... starting with off. */
    fun onOffTimings(segments: List<Segment>): LongArray {
        val out = ArrayList<Long>()
        var lastOn = false // the implicit leading slot is "off"
        out.add(0L)
        for (segment in segments) {
            val isOn = segment.amplitude > 0
            if (isOn == lastOn) out[out.lastIndex] = out.last() + segment.ms else out.add(segment.ms)
            lastOn = isOn
        }
        return out.toLongArray()
    }
}
