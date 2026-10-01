package sh.zeron.android.feedback

import android.os.VibrationEffect.Composition as C
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

class HapticDesignTest {
    private val allPrimitives = setOf(C.PRIMITIVE_CLICK, C.PRIMITIVE_TICK, C.PRIMITIVE_LOW_TICK, C.PRIMITIVE_THUD, C.PRIMITIVE_SPIN, C.PRIMITIVE_QUICK_RISE, C.PRIMITIVE_SLOW_RISE, C.PRIMITIVE_QUICK_FALL)

    private fun caps(sdk: Int = 36, primitives: Set<Int> = allPrimitives, amplitude: Boolean = true, view: Boolean = true) =
        HapticCapabilities(sdk, primitives, amplitude, view)

    @Test fun everyHapticHasASpec() {
        assertEquals(Haptic.entries.size, HapticTable.all.size)
        for (h in Haptic.entries) {
            val s = HapticTable.spec(h)
            assertEquals(h, s.haptic)
            assertTrue("$h has compositions", s.compositions.isNotEmpty())
            assertTrue("$h has a waveform", s.waveform.isNotEmpty() && s.waveform.any { it.amplitude > 0 })
            assertTrue("$h gap", s.minGapMs > 0)
        }
    }

    @Test fun everyHapticPlansOnEveryDevice() {
        val devices = listOf(
            caps(), caps(sdk = 29, primitives = emptySet(), view = true), caps(sdk = 29, primitives = emptySet(), amplitude = false, view = false),
            caps(sdk = 30, primitives = setOf(C.PRIMITIVE_CLICK, C.PRIMITIVE_TICK)), caps(sdk = 34, view = false),
        )
        for (h in Haptic.entries) for (strength in HapticStrength.entries) for (d in devices) {
            val plan = HapticPlanner.plan(h, strength, d)
            if (plan is HapticPlan.Skip) {
                assertTrue("only the lightest haptics may be dropped: $h $strength $d", strength == HapticStrength.Subtle && !d.amplitudeControl && HapticTable.spec(h).priority == 0)
            }
            if (plan is HapticPlan.Composition) assertTrue(plan.steps.all { it.primitive in d.primitives })
            if (plan is HapticPlan.Waveform) assertEquals(plan.timings.size, plan.amplitudes?.size ?: plan.timings.size)
        }
    }

    @Test fun standardPrefersThePlatformConstantWhereItFits() {
        assertEquals(HapticPlan.ViewConstant(android.view.HapticFeedbackConstants.SEGMENT_FREQUENT_TICK), HapticPlanner.plan(Haptic.Tick, HapticStrength.Standard, caps(sdk = 34)))
        assertEquals(HapticPlan.ViewConstant(android.view.HapticFeedbackConstants.CLOCK_TICK), HapticPlanner.plan(Haptic.Tick, HapticStrength.Standard, caps(sdk = 31)))
        assertEquals(HapticPlan.ViewConstant(android.view.HapticFeedbackConstants.TOGGLE_ON), HapticPlanner.plan(Haptic.ToggleOn, HapticStrength.Standard, caps(sdk = 34)))
        assertEquals(HapticPlan.ViewConstant(android.view.HapticFeedbackConstants.TOGGLE_OFF), HapticPlanner.plan(Haptic.ToggleOff, HapticStrength.Standard, caps(sdk = 34)))
        assertEquals(HapticPlan.ViewConstant(android.view.HapticFeedbackConstants.CONFIRM), HapticPlanner.plan(Haptic.Confirm, HapticStrength.Standard, caps(sdk = 30)))
    }

    @Test fun toggleOnAndOffDiffer() {
        // On rises (soft to crisp), off falls (crisp to soft): never the same pattern.
        for (d in listOf(caps(sdk = 31), caps(sdk = 29, primitives = emptySet()))) {
            assertFalse(HapticPlanner.plan(Haptic.ToggleOn, HapticStrength.Standard, d) == HapticPlanner.plan(Haptic.ToggleOff, HapticStrength.Standard, d))
        }
        val on = HapticTable.spec(Haptic.ToggleOn).compositions.first()
        val off = HapticTable.spec(Haptic.ToggleOff).compositions.first()
        assertTrue(on.last().scale > on.first().scale)
        assertTrue(off.last().scale < off.first().scale)
    }

    @Test fun compoundPatternsAreDesignedNotPlatformConstants() {
        for (h in listOf(Haptic.Success, Haptic.Attention, Haptic.Heavy)) {
            val plan = HapticPlanner.plan(h, HapticStrength.Standard, caps(sdk = 36))
            assertTrue("$h $plan", plan is HapticPlan.Composition && plan.steps.size >= 2)
        }
        // Error goes through the composition too; REJECT is only its fallback.
        assertTrue(HapticPlanner.plan(Haptic.Error, HapticStrength.Standard, caps()) is HapticPlan.Composition)
        assertEquals(HapticPlan.ViewConstant(android.view.HapticFeedbackConstants.REJECT), HapticPlanner.plan(Haptic.Error, HapticStrength.Standard, caps(primitives = emptySet())))
    }

    @Test fun strengthScalesPrimitives() {
        fun first(h: Haptic, s: HapticStrength) = (HapticPlanner.plan(h, s, caps(sdk = 34, view = false)) as HapticPlan.Composition).steps.first().scale
        val subtle = first(Haptic.Select, HapticStrength.Subtle)
        val standard = first(Haptic.Select, HapticStrength.Standard)
        val strong = first(Haptic.Select, HapticStrength.Strong)
        assertTrue("$subtle < $standard < $strong", subtle < standard && standard < strong)
        assertEquals(0.7f * 0.5f, subtle, 1e-6f)
        assertEquals(1f, HapticPlanner.scaled(0.9f, HapticStrength.Strong), 0f)
        assertEquals(0.05f, HapticPlanner.scaled(0.01f, HapticStrength.Subtle), 0f)
    }

    @Test fun subtleOrStrongNeverUseTheFixedStrengthPlatformConstant() {
        assertTrue(HapticPlanner.plan(Haptic.Tick, HapticStrength.Subtle, caps(sdk = 34)) is HapticPlan.Composition)
        assertTrue(HapticPlanner.plan(Haptic.Tick, HapticStrength.Strong, caps(sdk = 34)) is HapticPlan.Composition)
    }

    @Test fun waveformFallbackScalesAmplitude() {
        val none = caps(sdk = 29, primitives = emptySet(), view = false)
        val standard = HapticPlanner.plan(Haptic.Confirm, HapticStrength.Strong, none) as HapticPlan.Waveform
        val spec = HapticTable.spec(Haptic.Confirm)
        assertEquals((spec.waveform.first().amplitude * 1.5f).toInt(), standard.amplitudes!!.first())
        val subtle = HapticPlanner.plan(Haptic.Confirm, HapticStrength.Subtle, none) as HapticPlan.Waveform
        assertTrue(subtle.amplitudes!!.first() < spec.waveform.first().amplitude)
        assertTrue(standard.amplitudes!!.all { it in 0..255 })
    }

    @Test fun noAmplitudeControlUsesPredefinedAndDropsLightOnesWhenSubtle() {
        val basic = caps(sdk = 29, primitives = emptySet(), amplitude = false, view = false)
        assertTrue(HapticPlanner.plan(Haptic.Confirm, HapticStrength.Standard, basic) is HapticPlan.Predefined)
        assertTrue(HapticPlanner.plan(Haptic.Tick, HapticStrength.Subtle, basic) is HapticPlan.Skip)
        assertNotNull(HapticPlanner.plan(Haptic.Error, HapticStrength.Subtle, basic))
    }

    @Test fun onOffTimingsAlternateStartingOff() {
        val t = HapticPlanner.onOffTimings(listOf(Segment(10, 100), Segment(20, 0), Segment(5, 50)))
        assertEquals(listOf(0L, 10L, 20L, 5L), t.toList())
        val merged = HapticPlanner.onOffTimings(listOf(Segment(10, 100), Segment(5, 200)))
        assertEquals(listOf(0L, 15L), merged.toList())
    }

    @Test fun priorityOrdering() {
        assertTrue(HapticTable.spec(Haptic.Tick).priority < HapticTable.spec(Haptic.Confirm).priority)
        assertTrue(HapticTable.spec(Haptic.Confirm).priority < HapticTable.spec(Haptic.Error).priority)
    }
}
