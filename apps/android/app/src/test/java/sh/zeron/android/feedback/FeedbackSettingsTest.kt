package sh.zeron.android.feedback

import org.junit.Assert.assertEquals
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
}
