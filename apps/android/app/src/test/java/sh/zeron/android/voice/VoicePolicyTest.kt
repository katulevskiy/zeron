package sh.zeron.android.voice

import org.junit.Assert.*
import org.junit.Test

class VoicePolicyTest {
    private val phone = VoiceHost("phone", "Phone", true, true)
    private val desktop = VoiceHost("desktop", "Desktop", true, false)

    @Test fun disappearingSelectedHostNeverFallsBackToAnotherDevice() {
        assertEquals(phone, startHost(listOf(phone), null))
        assertNull(startHost(listOf(phone, desktop), null))
        assertNull(startHost(listOf(phone), desktop.id))
        assertNull(startHost(listOf(phone, desktop.copy(online = false)), desktop.id))
        assertEquals(desktop, startHost(listOf(phone, desktop), desktop.id))
    }

    @Test fun lateMediaPermissionAndServiceCallbacksCannotAffectNewCall() {
        val owner = VoiceGeneration()
        val first = owner.begin()
        assertTrue(owner.accepts(first))
        assertTrue(owner.end())
        assertFalse(owner.end())
        val second = owner.begin()
        assertFalse(owner.accepts(first))
        assertTrue(owner.accepts(second))
        owner.end()
        assertFalse(owner.accepts(second))
    }

    @Test fun localMuteWinsOverStaleUnmuteAndUnmuteWaitsForHost() {
        val mute = VoiceMute()
        mute.request(true)
        assertTrue(mute.effective(false))
        mute.request(false)
        assertTrue(mute.effective(true))
        assertFalse(mute.effective(false))
        mute.request(true)
        assertTrue(mute.effective(false))
    }

    @Test fun durationsRemainReadableAfterAnHour() {
        assertEquals("0:00", voiceElapsed(-1))
        assertEquals("1:01", voiceElapsed(61))
        assertEquals("1:01:01", voiceElapsed(3661))
    }

    @Test fun cameraPauseCannotBeUndoneByLateUnmuteAndPreservesUserMute() {
        val mute = VoiceMute()
        mute.cameraPaused = true
        assertTrue(mute.effective(false))
        mute.request(true)
        mute.cameraPaused = false
        assertTrue(mute.effective(false))
        mute.request(false)
        assertFalse(mute.effective(false))
    }
}
