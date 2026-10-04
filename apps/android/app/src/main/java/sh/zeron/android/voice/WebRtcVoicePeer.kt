package sh.zeron.android.voice

import android.content.Context
import android.media.AudioAttributes
import android.media.MediaRecorder
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.delay
import kotlinx.coroutines.withTimeout
import org.webrtc.*
import org.webrtc.audio.JavaAudioDeviceModule

/** Native audio and explicit photo context over the shared Codex V3 transport.
 * Never handles credentials, video streaming, or screen capture. */
class WebRtcVoicePeer(private val context: Context, private val failure: () -> Unit) {
    private var factory: PeerConnectionFactory? = null
    private var audio: JavaAudioDeviceModule? = null
    private var source: AudioSource? = null
    private var track: AudioTrack? = null
    private var peer: PeerConnection? = null
    private var channel: DataChannel? = null
    @Volatile private var closed = false
    private var activated = false
    private var maxMessageBytes = 65_536
    internal val maxPhotoBytes get() = (maxMessageBytes - 1024) / 4 * 3
    private val photos = VoicePhotoChannel { data ->
        !closed && activated && channel?.state() == DataChannel.State.OPEN &&
            (channel?.bufferedAmount() ?: Long.MAX_VALUE) == 0L &&
            channel?.send(DataChannel.Buffer(java.nio.ByteBuffer.wrap(data), false)) == true
    }

    fun prepare() {
        check(!closed)
        initialize(context)
        val onError = { if (!closed) failure() }
        val adm = JavaAudioDeviceModule.builder(context)
            .setAudioSource(MediaRecorder.AudioSource.VOICE_COMMUNICATION)
            .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
            .setUseHardwareAcousticEchoCanceler(true)
            .setUseHardwareNoiseSuppressor(true)
            .setEnableVolumeLogger(false)
            .setAudioRecordErrorCallback(object : JavaAudioDeviceModule.AudioRecordErrorCallback {
                override fun onWebRtcAudioRecordInitError(error: String) = onError()
                override fun onWebRtcAudioRecordStartError(code: JavaAudioDeviceModule.AudioRecordStartErrorCode, error: String) = onError()
                override fun onWebRtcAudioRecordError(error: String) = onError()
            })
            .setAudioTrackErrorCallback(object : JavaAudioDeviceModule.AudioTrackErrorCallback {
                override fun onWebRtcAudioTrackInitError(error: String) = onError()
                override fun onWebRtcAudioTrackStartError(code: JavaAudioDeviceModule.AudioTrackStartErrorCode, error: String) = onError()
                override fun onWebRtcAudioTrackError(error: String) = onError()
            }).createAudioDeviceModule()
        audio = adm
        // Negotiation cannot activate microphone capture or speaker output.
        // Use PeerConnection's recording gate. The SDK's setAudioRecordEnabled
        // chooses its internal/external recorder during initialization; flipping
        // it after negotiation can abort JNI because no AudioRecord was allocated.
        adm.setMicrophoneMute(true)
        adm.setSpeakerMute(true)
        val factory = PeerConnectionFactory.builder().setAudioDeviceModule(adm).createPeerConnectionFactory()
        this.factory = factory
        val config = PeerConnection.RTCConfiguration(emptyList()).apply {
            sdpSemantics = PeerConnection.SdpSemantics.UNIFIED_PLAN
            bundlePolicy = PeerConnection.BundlePolicy.MAXBUNDLE
        }
        val peer = checkNotNull(factory.createPeerConnection(config, observer))
        this.peer = peer
        peer.setAudioRecording(false)
        peer.setAudioPlayout(false)
        source = factory.createAudioSource(MediaConstraints())
        track = factory.createAudioTrack("voice-audio", source).also { it.setEnabled(false) }
        checkNotNull(peer.addTrack(track, listOf("voice")))
        channel = checkNotNull(peer.createDataChannel("oai-events", DataChannel.Init().apply { ordered = true }))
        channel?.registerObserver(object : DataChannel.Observer {
            override fun onBufferedAmountChange(previous: Long) = Unit
            override fun onStateChange() {
                if (!closed && channel?.state() in listOf(DataChannel.State.CLOSING, DataChannel.State.CLOSED)) failure()
            }
            // Canonical captions and tool handoffs come from the host coordinator.
            override fun onMessage(buffer: DataChannel.Buffer) {
                if (buffer.binary || buffer.data.remaining() > 262_144) return
                val bytes = ByteArray(buffer.data.remaining())
                buffer.data.get(bytes)
                photos.receive(bytes)
            }
        })
    }

    suspend fun offer(): String {
        val p = checkNotNull(peer)
        val offer = createSdp { p.createOffer(it, MediaConstraints()) }
        setSdp { p.setLocalDescription(it, offer) }
        waitUntil { p.iceGatheringState() == PeerConnection.IceGatheringState.COMPLETE }
        return checkNotNull(p.localDescription).description.also { require(it.toByteArray().size in 1..65_536) }
    }

    suspend fun applyAnswer(answer: String) {
        require(answer.toByteArray().size in 1..65_536)
        // SDP defaults to 64 KiB when omitted; 0 means unbounded. Keep our own
        // 256 KiB cap even when the remote endpoint advertises more.
        val advertised = Regex("(?m)^a=max-message-size:(\\d+)\\r?$").find(answer)?.groupValues?.get(1)?.toLongOrNull()
        maxMessageBytes = when (advertised) {
            null -> 65_536
            0L -> 262_144
            else -> advertised.coerceIn(0, 262_144).toInt()
        }
        val p = checkNotNull(peer)
        setSdp { p.setRemoteDescription(it, SessionDescription(SessionDescription.Type.ANSWER, answer)) }
        waitUntil { p.connectionState() == PeerConnection.PeerConnectionState.CONNECTED && channel?.state() == DataChannel.State.OPEN }
    }

    fun setMuted(muted: Boolean) {
        check(!closed && peer?.connectionState() == PeerConnection.PeerConnectionState.CONNECTED)
        activated = true
        audio?.setMicrophoneMute(muted)
        track?.setEnabled(!muted)
        peer?.setAudioRecording(!muted)
        audio?.setSpeakerMute(false)
        peer?.setAudioPlayout(true)
    }

    /** Mute closes capture immediately, independently of the coordinator queue. */
    fun muteLocally() {
        if (closed) return
        track?.setEnabled(false)
        audio?.setMicrophoneMute(true)
        peer?.setAudioRecording(false)
    }

    internal suspend fun addPhoto(jpeg: ByteArray) {
        check(!closed && activated) { "Start a voice call before adding a photo." }
        photos.add(jpeg, maxMessageBytes)
    }

    suspend fun levels(visible: Boolean): Pair<UShort, UShort> {
        if (!visible || !activated) return 0u.toUShort() to 0u.toUShort()
        check(!closed)
        val result = CompletableDeferred<RTCStatsReport>()
        checkNotNull(peer).getStats { result.complete(it) }
        val report = withTimeout(3_000) { result.await() }
        var microphone = 0.0
        var speaker = 0.0
        for (stat in report.statsMap.values) {
            val level = (stat.members["audioLevel"] as? Number)?.toDouble() ?: continue
            if (!level.isFinite()) continue
            if (stat.type == "media-source") microphone = maxOf(microphone, level)
            if (stat.type == "inbound-rtp") speaker = maxOf(speaker, level)
        }
        fun scaled(value: Double) = (value.coerceIn(0.0, 1.0) * 65535).toInt().toUShort()
        return scaled(microphone) to scaled(speaker)
    }

    fun close() {
        if (closed) return
        closed = true
        photos.close()
        track?.setEnabled(false)
        audio?.setMicrophoneMute(true)
        audio?.setSpeakerMute(true)
        peer?.setAudioRecording(false)
        peer?.setAudioPlayout(false)
        channel?.unregisterObserver()
        channel?.close()
        peer?.close()
        channel?.dispose(); channel = null
        // PeerConnection.dispose releases its sender and receiver track wrappers.
        peer?.dispose(); peer = null
        track?.dispose(); track = null
        source?.dispose(); source = null
        factory?.dispose(); factory = null
        audio?.release(); audio = null
    }

    private suspend fun waitUntil(ready: () -> Boolean) = withTimeout(25_000) {
        while (!ready()) { check(!closed); delay(20) }
        check(!closed)
    }

    private suspend fun createSdp(start: (SdpObserver) -> Unit): SessionDescription {
        val result = CompletableDeferred<SessionDescription>()
        start(object : SdpObserver {
            override fun onCreateSuccess(sdp: SessionDescription) { result.complete(sdp) }
            override fun onCreateFailure(error: String) { result.completeExceptionally(IllegalStateException("Voice offer unavailable")) }
            override fun onSetSuccess() = Unit
            override fun onSetFailure(error: String) = Unit
        })
        return withTimeout(25_000) { result.await() }
    }

    private suspend fun setSdp(start: (SdpObserver) -> Unit) {
        val result = CompletableDeferred<Unit>()
        start(object : SdpObserver {
            override fun onSetSuccess() { result.complete(Unit) }
            override fun onSetFailure(error: String) { result.completeExceptionally(IllegalStateException("Voice negotiation unavailable")) }
            override fun onCreateSuccess(sdp: SessionDescription) = Unit
            override fun onCreateFailure(error: String) = Unit
        })
        withTimeout(25_000) { result.await() }
    }

    private val observer = object : PeerConnection.Observer {
        override fun onSignalingChange(state: PeerConnection.SignalingState) = Unit
        override fun onIceConnectionChange(state: PeerConnection.IceConnectionState) {
            if (!closed && state in listOf(PeerConnection.IceConnectionState.FAILED, PeerConnection.IceConnectionState.DISCONNECTED, PeerConnection.IceConnectionState.CLOSED)) failure()
        }
        override fun onIceConnectionReceivingChange(receiving: Boolean) = Unit
        override fun onIceGatheringChange(state: PeerConnection.IceGatheringState) = Unit
        override fun onIceCandidate(candidate: IceCandidate) = Unit
        override fun onIceCandidatesRemoved(candidates: Array<out IceCandidate>) = Unit
        override fun onAddStream(stream: MediaStream) = Unit
        override fun onRemoveStream(stream: MediaStream) = Unit
        override fun onRenegotiationNeeded() = Unit
        override fun onDataChannel(channel: DataChannel) { channel.close() }
    }

    companion object {
        private var initialized = false
        @Synchronized private fun initialize(context: Context) {
            if (initialized) return
            PeerConnectionFactory.initialize(PeerConnectionFactory.InitializationOptions.builder(context.applicationContext)
                .setEnableInternalTracer(false).createInitializationOptions())
            initialized = true
        }
    }
}
