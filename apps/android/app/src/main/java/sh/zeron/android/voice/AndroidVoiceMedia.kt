package sh.zeron.android.voice

import android.content.Context
import kotlinx.coroutines.*
import uniffi.zeron_core.*
import java.lang.ref.WeakReference

/** One native audio lifetime per Rust call. Late callbacks cannot resurrect it. */
class AndroidVoiceMedia(
    private val context: Context,
    private val scope: CoroutineScope,
    private val generation: Long,
    private val permission: suspend () -> Boolean,
    private val visible: () -> Boolean,
    private val failure: () -> Unit,
) : VoiceMediaListener {
    private var call = WeakReference<VoiceCall>(null)
    private val jobs = mutableMapOf<ULong, Job>()
    private var closed = false
    private val started = android.os.SystemClock.elapsedRealtime()
    private val mute = VoiceMute()
    private val peer = WebRtcVoicePeer(context, failure)
    private val route = VoiceAudioRoute(context, failure)

    fun attach(call: VoiceCall) { this.call = WeakReference(call) }

    override fun onRequest(request: VoiceMediaRequest) {
        scope.launch {
            if (request.operation == VoiceMediaOperation.CLOSE) { close(); return@launch }
            if (closed) { complete(request, VoiceMediaFailure.UNAVAILABLE); return@launch }
            if (request.operation != VoiceMediaOperation.LEVELS && context.applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE != 0)
                android.util.Log.d("JarvisConnection", "${request.operation.name.lowercase()} elapsed_ms=${android.os.SystemClock.elapsedRealtime() - started}")
            if (jobs.size >= 4) { failure(); return@launch }
            val job = scope.launch(start = CoroutineStart.LAZY) {
                try {
                    var sdp: String? = null
                    var levels = 0u.toUShort() to 0u.toUShort()
                    when (request.operation) {
                        VoiceMediaOperation.PREPARE -> {
                            if (!permission()) { complete(request, VoiceMediaFailure.PERMISSION_DENIED); return@launch }
                            ensureActive()
                            JarvisService.ensureStarted(context, generation)
                            ensureActive()
                            route.prepare()
                            peer.prepare()
                        }
                        VoiceMediaOperation.OFFER -> sdp = peer.offer()
                        VoiceMediaOperation.APPLY_ANSWER -> peer.applyAnswer(checkNotNull(request.sdp))
                        VoiceMediaOperation.SET_MUTED -> { route.activate(); peer.setMuted(mute.effective(request.muted)) }
                        VoiceMediaOperation.LEVELS -> levels = peer.levels(visible())
                        VoiceMediaOperation.CLOSE -> Unit
                    }
                    ensureActive()
                    if (!closed) call.get()?.completeMedia(request.requestId, null, sdp,
                        if (mute.requested) 0u.toUShort() else levels.first, levels.second)
                } catch (e: CancellationException) { throw e }
                catch (_: Exception) { if (!closed) complete(request, VoiceMediaFailure.UNAVAILABLE) }
                finally { jobs.remove(request.requestId) }
            }
            jobs[request.requestId] = job
            job.start()
        }
    }

    private fun complete(request: VoiceMediaRequest, failure: VoiceMediaFailure) {
        call.get()?.completeMedia(request.requestId, failure, null, 0u.toUShort(), 0u.toUShort())
    }

    fun muteLocally(muted: Boolean) {
        mute.request(muted)
        if (muted && !closed) peer.muteLocally()
        // Unmute awaits the host's acknowledgement of current ownership.
    }

    fun toggleSpeaker() { if (!closed) route.toggleSpeaker() }


    fun close() {
        if (closed) return
        closed = true
        peer.close()
        route.close()
        jobs.values.toList().forEach { it.cancel() }
        jobs.clear()
        call.clear()
    }
}
