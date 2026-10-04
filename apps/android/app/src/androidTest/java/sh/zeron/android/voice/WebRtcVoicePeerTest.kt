package sh.zeron.android.voice

import android.Manifest
import android.content.Intent
import android.media.AudioManager
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.*
import org.junit.Assert.*
import org.junit.Test
import org.junit.Before
import org.junit.runner.RunWith
import org.webrtc.*
import sh.zeron.android.MainActivity

/** Real JNI/ICE/data-channel/audio-device test; no provider credentials or API calls. */
@RunWith(AndroidJUnit4::class)
class WebRtcVoicePeerTest {
    @Before fun foreground() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        // startActivitySync waits for an idle event queue; the live assistant
        // orb can continuously repaint. Wait for the actual resumed activity
        // instead, and dismiss any previous system overlay before this test.
        instrumentation.uiAutomation.executeShellCommand("input keyevent 4").close()
        instrumentation.runOnMainSync {
            context.startActivity(Intent(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK).putExtra("demo", true))
        }
        var resumed = false
        val deadline = System.currentTimeMillis() + 10_000
        while (!resumed && System.currentTimeMillis() < deadline) {
            instrumentation.runOnMainSync {
                resumed = androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry.getInstance()
                    .getActivitiesInStage(androidx.test.runner.lifecycle.Stage.RESUMED).any { it is MainActivity }
            }
            if (!resumed) Thread.sleep(50)
        }
        assertTrue("MainActivity must be resumed for Android's microphone/audio-focus policy", resumed)
        instrumentation.uiAutomation.grantRuntimePermission(context.packageName, Manifest.permission.RECORD_AUDIO)
    }
    @Test fun negotiationCannotCaptureUntilActivatedAndCloseReleasesDevices() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val audio = context.getSystemService(AudioManager::class.java)
        val initial = audio.activeRecordingConfigurations.size
        var failures = 0
        repeat(2) {
            val media = WebRtcVoicePeer(context) { failures++ }
            var factory: PeerConnectionFactory? = null
            var other: PeerConnection? = null
            var data: DataChannel? = null
            try {
                media.prepare()
                factory = PeerConnectionFactory.builder().createPeerConnectionFactory()
                other = factory.createPeerConnection(PeerConnection.RTCConfiguration(emptyList()).apply {
                    sdpSemantics = PeerConnection.SdpSemantics.UNIFIED_PLAN
                }, object : PeerConnection.Observer {
                    override fun onSignalingChange(state: PeerConnection.SignalingState) = Unit
                    override fun onIceConnectionChange(state: PeerConnection.IceConnectionState) = Unit
                    override fun onIceConnectionReceivingChange(receiving: Boolean) = Unit
                    override fun onIceGatheringChange(state: PeerConnection.IceGatheringState) = Unit
                    override fun onIceCandidate(candidate: IceCandidate) = Unit
                    override fun onIceCandidatesRemoved(candidates: Array<out IceCandidate>) = Unit
                    override fun onAddStream(stream: MediaStream) = Unit
                    override fun onRemoveStream(stream: MediaStream) = Unit
                    override fun onDataChannel(channel: DataChannel) {
                        data = channel
                        channel.registerObserver(object : DataChannel.Observer {
                            override fun onBufferedAmountChange(previous: Long) = Unit
                            override fun onStateChange() = Unit
                            override fun onMessage(buffer: DataChannel.Buffer) {
                                val bytes = ByteArray(buffer.data.remaining()).also { buffer.data.get(it) }
                                val event = org.json.JSONObject(bytes.toString(Charsets.UTF_8))
                                val item = event.getJSONObject("item")
                                assertEquals("input_image", item.getJSONArray("content").getJSONObject(0).getString("type"))
                                val reply = org.json.JSONObject().put("type", "conversation.item.added").put("item", item).toString().toByteArray()
                                channel.send(DataChannel.Buffer(java.nio.ByteBuffer.wrap(reply), false))
                            }
                        })
                    }
                    override fun onRenegotiationNeeded() = Unit
                })
                val peer = checkNotNull(other)
                val offer = media.offer()
                setSdp { peer.setRemoteDescription(it, SessionDescription(SessionDescription.Type.OFFER, offer)) }
                val answer = CompletableDeferred<SessionDescription>()
                peer.createAnswer(object : Observer() {
                    override fun onCreateSuccess(sdp: SessionDescription) { answer.complete(sdp) }
                    override fun onCreateFailure(error: String) { answer.completeExceptionally(IllegalStateException(error)) }
                }, MediaConstraints())
                val localAnswer = withTimeout(10_000) { answer.await() }
                setSdp { peer.setLocalDescription(it, localAnswer) }
                withTimeout(10_000) { while (peer.iceGatheringState() != PeerConnection.IceGatheringState.COMPLETE) delay(20) }
                media.applyAnswer(checkNotNull(peer.localDescription).description)
                assertEquals("Negotiation must not capture", initial, audio.activeRecordingConfigurations.size)
                media.setMuted(true)
                media.addPhoto(byteArrayOf(0xff.toByte(), 0xd8.toByte(), 0xff.toByte(), 0xd9.toByte()))
                assertEquals("Adding acknowledged photo context must not open the mic", initial, audio.activeRecordingConfigurations.size)
                media.setMuted(false)
                withTimeout(5_000) { while (audio.activeRecordingConfigurations.size <= initial) delay(50) }
                media.muteLocally()
                withTimeout(5_000) { while (audio.activeRecordingConfigurations.size != initial) delay(50) }
                media.setMuted(true)
                assertEquals(initial, audio.activeRecordingConfigurations.size)
                media.setMuted(false)
                media.levels(true)
            } finally {
                media.close()
                media.close() // teardown is idempotent
                data?.close(); data?.dispose()
                other?.close(); other?.dispose()
                factory?.dispose()
            }
            withTimeout(5_000) { while (audio.activeRecordingConfigurations.size != initial) delay(50) }
        }
        assertEquals(0, failures)
    }

    @Test fun closingRestoresAudioFocusAndCommunicationMode() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val audio = context.getSystemService(AudioManager::class.java)
        val before = audio.mode
        var failures = 0
        instrumentation.runOnMainSync {
            val route = VoiceAudioRoute(context) { failures++ }
            route.prepare()
            route.activate()
            assertEquals(AudioManager.MODE_IN_COMMUNICATION, audio.mode)
            route.toggleSpeaker()
            route.close()
            route.close()
        }
        assertEquals(before, audio.mode)
        assertEquals(0, failures)
    }

    private open class Observer : SdpObserver {
        override fun onCreateSuccess(sdp: SessionDescription) = Unit
        override fun onCreateFailure(error: String) = Unit
        override fun onSetSuccess() = Unit
        override fun onSetFailure(error: String) = Unit
    }
    private suspend fun setSdp(start: (SdpObserver) -> Unit) {
        val ready = CompletableDeferred<Unit>()
        start(object : Observer() {
            override fun onSetSuccess() { ready.complete(Unit) }
            override fun onSetFailure(error: String) { ready.completeExceptionally(IllegalStateException(error)) }
        })
        withTimeout(10_000) { ready.await() }
    }
}
