package sh.zeron.android.feedback

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FakeEnv(
    override var active: Boolean = true,
    override var systemTouchHaptics: Boolean = true,
    override var systemTouchSounds: Boolean = true,
    override var ringerNormal: Boolean = true,
    override var silencedByDnd: Boolean = false,
    override var hasVibrator: Boolean = true,
    override var streamAudible: Boolean = true,
) : FeedbackEnvironment

class CountingEnv : FeedbackEnvironment {
    var reads = 0
    override val active: Boolean get() = true.also { reads++ }
    override val systemTouchHaptics: Boolean get() = true.also { reads++ }
    override val systemTouchSounds: Boolean get() = true.also { reads++ }
    override val ringerNormal: Boolean get() = true.also { reads++ }
    override val silencedByDnd: Boolean get() = false.also { reads++ }
    override val hasVibrator: Boolean get() = true.also { reads++ }
    override val streamAudible: Boolean get() = true.also { reads++ }
}

class FeedbackGateTest {
    private var now = 10_000L
    private var settings = FeedbackSettings()
    private val env = FakeEnv()
    private val gate = FeedbackGate({ settings }, env, { now })

    private fun why(d: Decision) = (d as? Decision.Skip)?.why

    @Test fun defaultsPlayEverything() {
        assertEquals(Decision.Play, gate.haptic(Haptic.Confirm))
        assertEquals(Decision.Play, gate.cue(Cue.Send))
    }

    @Test fun hapticsSwitchAndSystemSettings() {
        settings = settings.copy(haptics = false)
        assertEquals(Skipped.HapticsOff, why(gate.haptic(Haptic.Confirm)))
        settings = FeedbackSettings()
        env.systemTouchHaptics = false
        assertEquals(Skipped.SystemHapticsOff, why(gate.haptic(Haptic.Confirm)))
        env.systemTouchHaptics = true
        env.hasVibrator = false
        assertEquals(Skipped.NoVibrator, why(gate.haptic(Haptic.Confirm)))
    }

    @Test fun hapticsStillPlayOnSilent() {
        env.ringerNormal = false
        env.silencedByDnd = true
        assertEquals(Decision.Play, gate.haptic(Haptic.Confirm))
    }

    @Test fun nothingPlaysInBackgroundOrScreenOff() {
        env.active = false
        assertEquals(Skipped.Inactive, why(gate.haptic(Haptic.Error)))
        assertEquals(Skipped.Inactive, why(gate.cue(Cue.Done)))
    }

    @Test fun soundsMasterAndCategories() {
        settings = settings.copy(sounds = false)
        assertEquals(Skipped.SoundsOff, why(gate.cue(Cue.Tap)))
        assertEquals(Skipped.SoundsOff, why(gate.cue(Cue.Done)))

        settings = FeedbackSettings(interfaceSounds = false)
        assertEquals(Skipped.CategoryOff, why(gate.cue(Cue.Tap)))
        assertEquals(Decision.Play, gate.cue(Cue.Done)) // session sounds are independent of interface sounds

        now += 1000
        settings = FeedbackSettings(completionSound = false)
        assertEquals(Skipped.CategoryOff, why(gate.cue(Cue.Done)))
        assertEquals(Decision.Play, gate.cue(Cue.Request))
        now += 1000
        settings = FeedbackSettings(inputSound = false)
        assertEquals(Skipped.CategoryOff, why(gate.cue(Cue.Request)))
        now += 1000
        settings = FeedbackSettings(errorSound = false)
        assertEquals(Skipped.CategoryOff, why(gate.cue(Cue.Attention)))
        assertEquals(Skipped.CategoryOff, why(gate.cue(Cue.Reconnected)))
        now += 1000
        settings = FeedbackSettings(sessionSounds = false)
        assertEquals(Skipped.CategoryOff, why(gate.cue(Cue.Done)))
        assertEquals(Skipped.CategoryOff, why(gate.cue(Cue.Request)))
        assertEquals(Skipped.CategoryOff, why(gate.cue(Cue.Attention)))
        assertEquals(Decision.Play, gate.cue(Cue.Tap))
    }

    @Test fun systemSoundState() {
        env.ringerNormal = false
        assertEquals(Skipped.Silent, why(gate.cue(Cue.Done)))
        env.ringerNormal = true
        env.silencedByDnd = true
        assertEquals(Skipped.Dnd, why(gate.cue(Cue.Done)))
        env.silencedByDnd = false
        env.streamAudible = false
        assertEquals(Skipped.Muted, why(gate.cue(Cue.Done)))
        env.streamAudible = true
        // Touch sounds off silences the interface layer only; session chimes are events.
        env.systemTouchSounds = false
        assertEquals(Skipped.SystemSoundsOff, why(gate.cue(Cue.Tap)))
        assertEquals(Decision.Play, gate.cue(Cue.Done))
    }

    @Test fun sameCueNeverStacks() {
        assertEquals(Decision.Play, gate.cue(Cue.Tap))
        now += 30
        assertEquals(Skipped.Rate, why(gate.cue(Cue.Tap)))
        now += 40
        assertEquals(Decision.Play, gate.cue(Cue.Tap))
    }

    @Test fun sameHapticIsCoalesced() {
        assertEquals(Decision.Play, gate.haptic(Haptic.Tick))
        now += 20
        assertEquals(Skipped.Rate, why(gate.haptic(Haptic.Tick)))
        now += 60
        assertEquals(Decision.Play, gate.haptic(Haptic.Tick))
    }

    @Test fun lightHapticsDoNotChatterAcrossKinds() {
        assertEquals(Decision.Play, gate.haptic(Haptic.Tick))
        now += 10
        assertEquals(Skipped.Rate, why(gate.haptic(Haptic.Select))) // inside the global gap
        now += 30
        assertEquals(Decision.Play, gate.haptic(Haptic.Select))
    }

    @Test fun importantHapticsCutThroughLightOnes() {
        assertEquals(Decision.Play, gate.haptic(Haptic.Tick))
        now += 5
        assertEquals(Decision.Play, gate.haptic(Haptic.Error))
        now += 5
        assertEquals(Skipped.Rate, why(gate.haptic(Haptic.Tick)))
    }

    @Test fun burstOfLightHapticsIsCapped() {
        var played = 0
        repeat(60) {
            if (gate.haptic(if (it % 2 == 0) Haptic.Tick else Haptic.Select) == Decision.Play) played++
            now += 30
        }
        // 60 events over 1.8 s, 12 per second at most.
        assertTrue("played $played", played <= FeedbackGate.LIGHT_HAPTICS_PER_WINDOW * 2)
        assertTrue(played > 0)
    }

    @Test fun streamsAreCapped() {
        // Four long cues of rising priority at distinct times fill the pool; a fifth interface cue is refused.
        val long = listOf(Cue.Done, Cue.Request, Cue.UploadReady, Cue.Reconnected)
        for (cue in long) {
            assertEquals(Decision.Play, gate.cue(cue))
            now += 30
        }
        assertEquals(Skipped.Streams, why(gate.cue(Cue.Tap)))
        // A more important cue still gets in (steals a stream).
        assertEquals(Decision.Play, gate.cue(Cue.Attention))
        now += 2000
        assertEquals(Decision.Play, gate.cue(Cue.Tap))
    }

    @Test fun playAnywayIgnoresTheSystemTouchSoundsFlag() {
        env.systemTouchSounds = false
        assertEquals(Skipped.SystemSoundsOff, why(gate.cue(Cue.Tap)))
        settings = settings.copy(ignoreSystemTouchSounds = true)
        now += 1000
        assertEquals(Decision.Play, gate.cue(Cue.Tap))
        // Only that one flag is overridden: silent, Do Not Disturb, a zero volume, the switches and an inactive app still win.
        now += 1000
        env.ringerNormal = false
        assertEquals(Skipped.Silent, why(gate.cue(Cue.Tap)))
        env.ringerNormal = true
        env.silencedByDnd = true
        assertEquals(Skipped.Dnd, why(gate.cue(Cue.Tap)))
        env.silencedByDnd = false
        env.streamAudible = false
        assertEquals(Skipped.Muted, why(gate.cue(Cue.Tap)))
        env.streamAudible = true
        env.active = false
        assertEquals(Skipped.Inactive, why(gate.cue(Cue.Tap)))
        env.active = true
        settings = settings.copy(interfaceSounds = false)
        assertEquals(Skipped.CategoryOff, why(gate.cue(Cue.Tap)))
    }

    @Test fun playAnywayDoesNotTouchHaptics() {
        env.systemTouchHaptics = false
        settings = settings.copy(ignoreSystemTouchSounds = true)
        assertEquals(Skipped.SystemHapticsOff, why(gate.haptic(Haptic.Confirm)))
    }

    @Test fun theGateReadsTheEnvironmentOncePerDecisionNotPerProperty() {
        // Each decision touches each property a bounded number of times: the real environment answers from a
        // cached snapshot, and the gate must not multiply even those reads.
        val counting = CountingEnv()
        val g = FeedbackGate({ settings }, counting, { now })
        g.cue(Cue.Tap)
        assertTrue("reads=${counting.reads}", counting.reads <= 7)
        counting.reads = 0
        now += 1000
        g.haptic(Haptic.Confirm)
        assertTrue("reads=${counting.reads}", counting.reads <= 3)
    }

}
