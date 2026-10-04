package sh.zeron.android.voice

import android.Manifest
import android.content.Intent
import android.media.AudioManager
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.viewinterop.AndroidView
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import sh.zeron.android.voice.assistant.*
import uniffi.zeron_core.*
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
        android.os.ParcelFileDescriptor.AutoCloseInputStream(instrumentation.uiAutomation.executeShellCommand("input keyevent 4")).use { it.readBytes() }
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
                media.setMuted(false)
                withTimeout(5_000) { while (audio.activeRecordingConfigurations.size <= initial) delay(50) }
                media.muteLocally()
                withTimeout(5_000) { while (audio.activeRecordingConfigurations.size != initial) delay(50) }
                media.setMuted(true)
                assertEquals(initial, audio.activeRecordingConfigurations.size)
                media.setMuted(false)
                media.levels(true)
                if (it == 1) captureInCircleWhileAudioRemainsActive(media, audio, initial, instrumentation)
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

    private suspend fun captureInCircleWhileAudioRemainsActive(
        media: WebRtcVoicePeer, audio: AudioManager, initial: Int,
        instrumentation: android.app.Instrumentation,
    ) {
        val context = instrumentation.targetContext
        instrumentation.uiAutomation.grantRuntimePermission(context.packageName, Manifest.permission.CAMERA)
        var activity: MainActivity? = null
        instrumentation.runOnMainSync { activity = ActivityLifecycleMonitorRegistry.getInstance().getActivitiesInStage(Stage.RESUMED).filterIsInstance<MainActivity>().firstOrNull() }
        val screen = checkNotNull(activity)
        val captured = CompletableDeferred<java.io.File>()
        lateinit var camera: JarvisOverlayCamera
        val state = JarvisState(live = true, call = VoiceCallState(VoiceCallPhase.ACTIVE, VoiceOrb.LISTENING,
            "camera-audio-test", VoiceCallWork.IDLE, false, false, "Keep talking while taking a photo", null, null, .2f, 0f, emptyList()))
        try {
            instrumentation.runOnMainSync {
                camera = JarvisOverlayCamera(context, screen, { captured.complete(it) }, { captured.completeExceptionally(IllegalStateException(it)) })
                screen.setContent {
                    val preview by camera.state.collectAsState()
                    Box(Modifier.fillMaxSize().background(Color(0xFF151518))) {
                        JarvisAssistantContent(state, false, null, null, false, true, camera = preview,
                            cameraPreview = { AndroidView(factory = { androidx.camera.view.PreviewView(it).also(camera::attach) }, modifier = Modifier.fillMaxSize()) }) {
                            if (it == AssistantAction.Shutter) camera.shutter()
                        }
                    }
                }
                camera.open()
            }
            withTimeout(15_000) { while (!camera.state.value.ready) { assertTrue("Voice recording must continue while the camera loads", audio.activeRecordingConfigurations.size > initial); delay(50) } }
            delay(400) // let the first-frame iris opening settle
            instrumentation.uiAutomation.takeScreenshot().let { bitmap ->
                java.io.File(context.filesDir, "jarvis-circle-camera-test.png").outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }
                bitmap.recycle()
            }
            val start = android.os.SystemClock.elapsedRealtime()
            instrumentation.runOnMainSync { camera.shutter(); camera.shutter() /* no duplicate */ }
            withTimeout(10_000) { while (!captured.isCompleted) { assertTrue("Shutter must not stop microphone capture", audio.activeRecordingConfigurations.size > initial); delay(20) } }
            val file = captured.await()
            try {
                assertTrue(file.length() > 0)
                val jpeg = VoicePhotoEncoder.encode(file, 768_000)
                assertNotNull(android.graphics.BitmapFactory.decodeByteArray(jpeg, 0, jpeg.size))
                assertFalse(camera.state.value.open)
                assertTrue("Audio still records after capture/encoding", audio.activeRecordingConfigurations.size > initial)
                media.levels(true)
                android.util.Log.i("JarvisCameraTest", "shutter_to_saved_ms=${android.os.SystemClock.elapsedRealtime() - start}")
            } finally { file.delete() }
        } finally {
            instrumentation.runOnMainSync { camera.dispose(); screen.setContent { Box(Modifier.fillMaxSize()) } }
        }
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
