package sh.zeron.android.voice.assistant

import android.content.Intent
import android.os.Bundle
import android.service.voice.VoiceInteractionService
import android.service.voice.VoiceInteractionSession
import android.service.voice.VoiceInteractionSessionService
import sh.zeron.android.ZeronApplication

/** Lightweight registration only: selection never boots Jarvis or opens a mic. */
class JarvisAssistantService : VoiceInteractionService() {
    override fun onReady() {
        super.onReady()
        // Disable both kinds of screen context even when the OS supplies flags
        // for a hardware invocation. Jarvis receives microphone audio only.
        setDisabledShowContext(VoiceInteractionSession.SHOW_WITH_ASSIST or VoiceInteractionSession.SHOW_WITH_SCREENSHOT)
    }
    override fun onShutdown() {
        (application as ZeronApplication).existingModel?.jarvis?.assistantDisabled()
        super.onShutdown()
    }
}

class JarvisAssistantSessionService : VoiceInteractionSessionService() {
    override fun onNewSession(args: Bundle?): VoiceInteractionSession = JarvisAssistantSession(this)
}

