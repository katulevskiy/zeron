package sh.zeron.android.voice.screen

import android.content.Intent
import android.media.AudioManager
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.Assert.*
import sh.zeron.android.ZeronApplication
import sh.zeron.android.voice.*
import uniffi.zeron_core.*

/** Real platform capture/consent and real JNI microphone; the call state is a
 * fixture, so no screenshot is uploaded and no OpenAI session is started. */
internal suspend fun verifyProjectionWithLiveMicrophone(audio: AudioManager, initial: Int) {
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val context = instrumentation.targetContext
    val model = (context.applicationContext as ZeronApplication).model
    withTimeout(10_000) { while (model.client.value == null) delay(50) }
    val controller = model.jarvis
    val screen = controller.screen
    val oldContext = screen.settings.contextEnabled.value
    var sharingStarted = false
    try {
        withContext(Dispatchers.Main) {
            controller.stop()
            screen.setContext(true)
            val generationField = JarvisController::class.java.getDeclaredField("generation").apply { isAccessible = true }
            val id = (generationField.get(controller) as VoiceGeneration).begin()
            val stateField = JarvisController::class.java.getDeclaredField("_state").apply { isAccessible = true }
            @Suppress("UNCHECKED_CAST")
            val state = stateField.get(controller) as MutableStateFlow<JarvisState>
            state.value = JarvisState(live = true, call = VoiceCallState(VoiceCallPhase.ACTIVE, VoiceOrb.LISTENING, null,
                VoiceCallWork.IDLE, false, false, "", null, null, 0f, 0f, emptyList()))
            context.startActivity(Intent(context, ScreenProjectionActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            assertTrue(controller.owns(id))
        }
        fun nodes(): List<AccessibilityNodeInfo> {
            val result = mutableListOf<AccessibilityNodeInfo>()
            fun visit(node: AccessibilityNodeInfo) { result.add(node); for (i in 0 until node.childCount) node.getChild(i)?.let(::visit) }
            instrumentation.uiAutomation.rootInActiveWindow?.let(::visit)
            return result
        }
        fun click(node: AccessibilityNodeInfo): Boolean {
            var target = node
            while (!target.isClickable && target.parent != null) target = target.parent
            return target.performAction(AccessibilityNodeInfo.ACTION_CLICK)
        }
        withTimeout(15_000) {
            var entireSelected = false
            while (!screen.state.value.sharing) {
                val visible = nodes()
                val single = visible.firstOrNull { it.text?.toString() == "A single app" }
                val entire = visible.firstOrNull { it.text?.toString() == "Entire screen" }
                if (!entireSelected && entire != null) { click(entire); entireSelected = true }
                else if (!entireSelected && single != null) click(single)
                else visible.firstOrNull { it.isEnabled && it.text?.toString() in listOf("Start now", "Start", "Share screen") }?.let(::click)
                assertTrue("Consent must preserve recording", audio.activeRecordingConfigurations.size > initial)
                delay(100)
            }
        }
        sharingStarted = true
        val frameField = ScreenController::class.java.getDeclaredField("latestFrame").apply { isAccessible = true }
        suspend fun frameHash(): Int? = withContext(Dispatchers.Main) {
            synchronized(screen) {
                val frame = frameField.get(screen) as? android.graphics.Bitmap ?: return@synchronized null
                val thumb = android.graphics.Bitmap.createScaledBitmap(frame,24,24,false)
                val data = IntArray(576); thumb.getPixels(data,0,24,0,0,24,24)
                if (thumb !== frame) thumb.recycle()
                data.contentHashCode()
            }
        }
        var before: Int? = null
        withTimeout(5000) { while (before == null) { before = frameHash(); delay(50) } }
        withContext(Dispatchers.Main) { context.startActivity(Intent(android.provider.Settings.ACTION_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
        withTimeout(5000) { while (frameHash() == before) { assertTrue(audio.activeRecordingConfigurations.size > initial); delay(50) } }
        assertTrue(screen.state.value.sharing)
        assertTrue("Capture must not stop the live microphone",audio.activeRecordingConfigurations.size > initial)
        withContext(Dispatchers.Main) { screen.stopSharing() }
        withTimeout(5000) { while (frameField.get(screen) != null) delay(50) }
        assertFalse(screen.state.value.sharing)
        assertTrue("Stopping screen sharing must preserve microphone",audio.activeRecordingConfigurations.size > initial)
        android.util.Log.i("JarvisScreenValidation","projection consent=true changed_frames=true microphone_preserved=true cleanup=true")
    } finally {
        withContext(Dispatchers.Main) { screen.stopSharing(); screen.setContext(oldContext); controller.stop() }
        if (!sharingStarted) instrumentation.uiAutomation.rootInActiveWindow?.let { root ->
            root.findAccessibilityNodeInfosByText("Cancel").firstOrNull()?.performAction(AccessibilityNodeInfo.ACTION_CLICK)
        }
        withContext(Dispatchers.Main) {
            context.startActivity(Intent(context, sh.zeron.android.MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK).putExtra("demo", true))
        }
        withTimeout(5000) {
            while (!withContext(Dispatchers.Main) {
                androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry.getInstance()
                    .getActivitiesInStage(androidx.test.runner.lifecycle.Stage.RESUMED).any { it is sh.zeron.android.MainActivity }
            }) delay(50)
        }
    }
}
