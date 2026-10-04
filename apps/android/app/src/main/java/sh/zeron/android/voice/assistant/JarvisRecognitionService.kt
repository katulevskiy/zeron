package sh.zeron.android.voice.assistant

import android.content.ComponentName
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.os.Bundle
import android.provider.Settings
import android.speech.RecognitionListener
import android.speech.RecognitionService
import android.speech.SpeechRecognizer
import sh.zeron.android.ZeronApplication

/** Android requires this component in assistant metadata. Android 10/11 can
 * also select it as the default SpeechRecognizer, so preserve ordinary OS
 * dictation by delegating to an installed provider rather than failing it or
 * routing unrelated speech requests to Jarvis/OpenAI. No recognizer intent
 * filter is advertised: newer Android keeps its original speech provider. */
class JarvisRecognitionService : RecognitionService() {
    private var recognizer: SpeechRecognizer? = null
    private var pending: Callback? = null

    override fun onStartListening(intent: Intent, listener: Callback) {
        close()
        if ((application as ZeronApplication).existingModel?.jarvis?.state?.value?.live == true) {
            listener.error(SpeechRecognizer.ERROR_RECOGNIZER_BUSY)
            return
        }
        val providers = packageManager.queryIntentServices(Intent(SERVICE_INTERFACE), 0)
            .map { it.serviceInfo }
            .filter { it.exported && (it.permission == null || it.permission == android.Manifest.permission.RECORD_AUDIO) && it.packageName != packageName }
        val preferred = Settings.Secure.getString(contentResolver, "voice_recognition_service")
            ?.let(ComponentName::unflattenFromString)
        val provider = providers.firstOrNull { ComponentName(it.packageName, it.name) == preferred }
            ?: providers.firstOrNull { it.applicationInfo.flags and ApplicationInfo.FLAG_SYSTEM != 0 }
            ?: providers.firstOrNull()
        if (provider == null) { listener.error(SpeechRecognizer.ERROR_CLIENT); return }
        pending = listener
        val delegate = SpeechRecognizer.createSpeechRecognizer(this, ComponentName(provider.packageName, provider.name))
        recognizer = delegate
        delegate.setRecognitionListener(object : RecognitionListener {
            private fun send(block: Callback.() -> Unit) {
                if (pending !== listener) return
                try { listener.block() } catch (_: android.os.RemoteException) { close() }
            }
            override fun onReadyForSpeech(params: Bundle?) = send { readyForSpeech(params ?: Bundle()) }
            override fun onBeginningOfSpeech() = send { beginningOfSpeech() }
            override fun onRmsChanged(rmsdB: Float) = send { rmsChanged(rmsdB) }
            override fun onBufferReceived(buffer: ByteArray) = send { bufferReceived(buffer) }
            override fun onEndOfSpeech() = send { endOfSpeech() }
            override fun onPartialResults(results: Bundle) = send { partialResults(results) }
            override fun onEvent(eventType: Int, params: Bundle?) = Unit
            override fun onError(error: Int) { send { error(error) }; if (pending === listener) close() }
            override fun onResults(results: Bundle) { send { results(results) }; if (pending === listener) close() }
        })
        delegate.startListening(intent)
    }
    override fun onStopListening(listener: Callback) { recognizer?.stopListening() }
    override fun onCancel(listener: Callback) { close() }
    override fun onDestroy() { close(); super.onDestroy() }
    private fun close() { pending = null; recognizer?.destroy(); recognizer = null }
}
