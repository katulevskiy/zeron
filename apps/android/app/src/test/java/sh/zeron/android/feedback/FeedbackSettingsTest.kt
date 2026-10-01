package sh.zeron.android.feedback

import kotlin.math.log10
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FeedbackSettingsTest {
    @Test fun defaultsAreOnAndRestrained() {
        val d = FeedbackSettings()
        assertEquals(true, d.sounds && d.interfaceSounds && d.sessionSounds && d.completionSound && d.inputSound && d.errorSound && d.haptics)
        assertEquals(HapticStrength.Standard, d.strength)
    }

    @Test fun roundTrips() {
        val s = FeedbackSettings(sounds = false, interfaceSounds = false, sessionSounds = false, completionSound = false, inputSound = false, errorSound = false, volume = 0.4f, haptics = false, strength = HapticStrength.Strong)
        val stored = FeedbackSettingsCodec.write(s)
        assertEquals(s, FeedbackSettingsCodec.read { stored[it] })
    }

    @Test fun missingOrMalformedReadAsDefaults() {
        assertEquals(FeedbackSettings(), FeedbackSettingsCodec.read { null })
        val bad = mapOf<String, Any>("feedback.volume" to Float.NaN, "feedback.strength" to "Thunder", "feedback.sounds" to "yes")
        assertEquals(FeedbackSettings(), FeedbackSettingsCodec.read { bad[it] })
        val loud = mapOf<String, Any>("feedback.volume" to 9f)
        assertEquals(1f, FeedbackSettingsCodec.read { loud[it] }.volume, 0f)
    }

    @Test fun categorySwitchesAreIndependentUnderTheMaster() {
        val s = FeedbackSettings(completionSound = false)
        assertEquals(false, s.allows(CueCategory.Completion))
        assertEquals(true, s.allows(CueCategory.Input))
        assertEquals(true, s.allows(CueCategory.Errors))
        assertEquals(true, s.allows(CueCategory.Interface))
        assertEquals(false, FeedbackSettings(sounds = false).allows(CueCategory.Interface))
    }

    // --- volume model: the slider's 100% is twice the old maximum, the default (50%) is the old maximum

    @Test fun defaultIsHalfWayAndPlaysAtTheLoudnessTheAppAlwaysHad() {
        assertEquals(0.5f, FeedbackSettings().volume, 0f)
        assertEquals(1f, FeedbackSettings().gain, 1e-6f)
    }

    @Test fun fullSliderIsSixDecibelsLouderThanTheDefault() {
        val max = FeedbackSettings(volume = 1f).gain
        assertEquals(FeedbackSettings.MAX_GAIN, max, 1e-6f)
        assertEquals(6.02, 20 * log10(max.toDouble()), 0.01)
        assertEquals(0f, FeedbackSettings(volume = 0f).gain, 0f)
        assertEquals(FeedbackSettings.MAX_GAIN, FeedbackSettings(volume = 7f).gain, 1e-6f) // clamped
    }

    @Test fun curveIsMonotoneContinuousAndSquaredBelowTheDefault() {
        var last = -1f
        for (i in 0..100) {
            val g = FeedbackSettings.gainFor(i / 100f)
            assertTrue("$i%", g > last || i == 0)
            last = g
        }
        // Below the default the curve is the one the app always had: gain = (2 * slider)^2.
        assertEquals(0.25f, FeedbackSettings.gainFor(0.25f), 1e-6f) // slider 25% = old 50% = -12 dB
        assertEquals(0.0625f, FeedbackSettings.gainFor(0.125f), 1e-6f)
        // No jump at the join.
        assertEquals(FeedbackSettings.gainFor(0.5f), FeedbackSettings.gainFor(0.5001f), 1e-3f)
        // Above it every step is the same number of decibels.
        val step = 20 * log10(FeedbackSettings.gainFor(0.75f).toDouble() / FeedbackSettings.gainFor(0.5f))
        assertEquals(3.01, step, 0.01)
    }

    @Test fun assetBoostLeavesTheDefaultLoudnessAndMakesTheMaximumTwiceAsLoud() {
        val spec = CueTable.spec(Cue.Tap)
        // Assets carry +6 dB; SoundPool volume is capped at 1.0. Default plays them at half = the old level.
        assertEquals(0.5f, CueTable.volume(spec, FeedbackSettings().gain), 1e-6f)
        assertEquals(1f, CueTable.volume(spec, FeedbackSettings(volume = 1f).gain), 1e-6f)
        // The maximum never asks SoundPool for more than 1.0, even for a trim of 1.
        for (s in CueTable.all) assertTrue(CueTable.volume(s, FeedbackSettings.MAX_GAIN) <= 1f)
        // Relative to what the app played before (volume = old gain * trim, assets at the old level):
        for (s in CueTable.all) {
            val before = 1f * s.gain
            val now = CueTable.volume(s, FeedbackSettings().gain) * CueTable.ASSET_BOOST
            assertEquals(s.cue.name, before, now, 1e-6f)
        }
    }

    // --- migration of the stored volume

    @Test fun oldStoredVolumeIsHalvedSoTheSameLoudnessIsKept() {
        for (old in listOf(0f, 0.1f, 0.4f, 0.7f, 1f)) {
            val stored = mutableMapOf<String, Any>("feedback.volume" to old, "feedback.sounds" to true)
            val updates = FeedbackSettingsCodec.migrate { stored[it] }
            stored.putAll(updates)
            val volume = FeedbackSettingsCodec.read { stored[it] }.volume
            assertEquals(old / 2, volume, 1e-6f)
            // The old gain was old^2; the new curve at the halved slider gives exactly that.
            assertEquals(old * old, FeedbackSettings.gainFor(volume), 1e-6f)
        }
    }

    @Test fun migrationRunsOnce() {
        val stored = mutableMapOf<String, Any>("feedback.volume" to 0.8f)
        stored.putAll(FeedbackSettingsCodec.migrate { stored[it] })
        assertEquals(0.4f, stored["feedback.volume"] as Float, 1e-6f)
        assertTrue(FeedbackSettingsCodec.migrate { stored[it] }.isEmpty())
        assertEquals(0.4f, FeedbackSettingsCodec.read { stored[it] }.volume, 1e-6f)
    }

    @Test fun freshInstallGetsTheNewDefaultAndIsJustStamped() {
        val updates = FeedbackSettingsCodec.migrate { null }
        assertEquals(setOf("feedback.prefs_version"), updates.keys)
        val stored = updates
        assertEquals(0.5f, FeedbackSettingsCodec.read { stored[it] }.volume, 0f)
    }

    @Test fun writtenSettingsCarryTheCurrentVersionSoTheyAreNeverMigratedAgain() {
        val stored = FeedbackSettingsCodec.write(FeedbackSettings(volume = 0.9f))
        assertTrue(FeedbackSettingsCodec.migrate { stored[it] }.isEmpty())
        assertEquals(0.9f, FeedbackSettingsCodec.read { stored[it] }.volume, 0f)
    }

    @Test fun malformedStoredVolumeMigratesToTheDefault() {
        val stored = mapOf<String, Any>("feedback.volume" to Float.NaN)
        val updates = FeedbackSettingsCodec.migrate { stored[it] }
        assertTrue("feedback.volume" !in updates)
        assertEquals(0.5f, FeedbackSettingsCodec.read { (stored + updates)[it] }.volume, 0f)
    }

    @Test fun overrideRoundTrips() {
        val s = FeedbackSettings(ignoreSystemTouchSounds = true)
        val stored = FeedbackSettingsCodec.write(s)
        assertEquals(true, FeedbackSettingsCodec.read { stored[it] }.ignoreSystemTouchSounds)
        assertEquals(false, FeedbackSettings().ignoreSystemTouchSounds)
    }

    // --- the Settings warning

    @Test fun touchSoundsWarningNamesTheOverrideState() {
        val env = FakeEnv(systemTouchSounds = false)
        val off = SystemNotes.build(env, FeedbackSettings())
        assertEquals(listOf(SystemNote.Kind.TouchSounds), off.map { it.kind })
        assertTrue(off.single().text.contains("muted"))
        val anyway = SystemNotes.build(env, FeedbackSettings(ignoreSystemTouchSounds = true)).single()
        assertEquals(SystemNote.Kind.TouchSounds, anyway.kind)
        assertTrue(anyway.text.contains("anyway"))
    }

    @Test fun noTouchSoundsWarningWhenNothingWouldPlayAnyway() {
        val env = FakeEnv(systemTouchSounds = false)
        assertTrue(SystemNotes.build(env, FeedbackSettings(interfaceSounds = false)).isEmpty())
        assertTrue(SystemNotes.build(env, FeedbackSettings(sounds = false)).isEmpty())
        // A silent phone explains itself first: the touch-sounds line would only add noise.
        val silent = SystemNotes.build(FakeEnv(systemTouchSounds = false, ringerNormal = false), FeedbackSettings())
        assertEquals(listOf(SystemNote.Kind.Info), silent.map { it.kind })
        assertTrue(SystemNotes.build(FakeEnv(), FeedbackSettings()).isEmpty())
    }
}
