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
        current = java.lang.ref.WeakReference(this)
        updateContext()
    }
    private fun updateContext() {
        val enabled = getSharedPreferences("jarvis-screen", 0).getBoolean("context", false)
        setDisabledShowContext(VoiceInteractionSession.SHOW_WITH_ASSIST or if (enabled) 0 else VoiceInteractionSession.SHOW_WITH_SCREENSHOT)
    }
    companion object {
        private var current: java.lang.ref.WeakReference<JarvisAssistantService>? = null
        fun refreshContext() { current?.get()?.updateContext(); JarvisAssistantSession.refreshContext() }
    }
    override fun onShutdown() {
        if (current?.get() === this) current = null
        (application as ZeronApplication).existingModel?.jarvis?.assistantDisabled()
        super.onShutdown()
    }
}

class JarvisAssistantSessionService : VoiceInteractionSessionService() {
    override fun onNewSession(args: Bundle?): VoiceInteractionSession = JarvisAssistantSession(this)
}

