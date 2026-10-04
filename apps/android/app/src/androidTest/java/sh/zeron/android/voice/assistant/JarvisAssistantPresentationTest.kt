package sh.zeron.android.voice.assistant

import android.content.Intent
import android.graphics.Rect
import android.view.accessibility.AccessibilityNodeInfo
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import sh.zeron.android.MainActivity
import sh.zeron.android.voice.JarvisState
import uniffi.zeron_core.*
import java.util.concurrent.atomic.AtomicReference

@RunWith(AndroidJUnit4::class)
class JarvisAssistantPresentationTest {
    @Test fun minimalLiveStageHasAccessibleCornerActionsAndNoExtraLabels() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        android.os.ParcelFileDescriptor.AutoCloseInputStream(instrumentation.uiAutomation.executeShellCommand("input keyevent 4")).use { it.readBytes() }
        instrumentation.runOnMainSync {
            context.startActivity(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK).putExtra("demo", true))
        }
        var activity: MainActivity? = null
        val until = System.currentTimeMillis() + 10_000
        while (activity == null && System.currentTimeMillis() < until) {
            instrumentation.runOnMainSync { activity = ActivityLifecycleMonitorRegistry.getInstance().getActivitiesInStage(Stage.RESUMED).filterIsInstance<MainActivity>().firstOrNull() }
            if (activity == null) Thread.sleep(50)
        }
        val state = JarvisState(live = true, host = "HOST_LABEL_MUST_NOT_APPEAR", call = VoiceCallState(
            VoiceCallPhase.ACTIVE, VoiceOrb.LISTENING, "voice-test", VoiceCallWork.IDLE, false, false,
            "A caption stays readable", null, null, 0.2f, 0f, emptyList()))
        val action = AtomicReference<AssistantAction?>(null)
        val screen = checkNotNull(activity)
        fun show(busy: Boolean) = instrumentation.runOnMainSync {
            screen.setContent {
                Box(Modifier.fillMaxSize().background(Color(0xFF151518))) {
                    JarvisAssistantContent(state, false, null, null, busy, true, onAction = action::set)
                }
            }
        }
        fun descendants(root: AccessibilityNodeInfo?): List<AccessibilityNodeInfo> =
            if (root == null) emptyList() else listOf(root) + (0 until root.childCount).flatMap { descendants(root.getChild(it)) }
        fun nodes(): List<AccessibilityNodeInfo> = descendants(instrumentation.uiAutomation.rootInActiveWindow)
        fun find(description: String): AccessibilityNodeInfo {
            val deadline = System.currentTimeMillis() + 5_000
            while (System.currentTimeMillis() < deadline) {
                nodes().firstOrNull { it.contentDescription?.toString() == description }?.let { return it }
                Thread.sleep(50)
            }
            error("Missing accessible control: $description")
        }
        fun button(description: String): AccessibilityNodeInfo {
            var node = find(description)
            while (node.isEnabled && !node.isClickable &&
                node.actionList.none { it.id == AccessibilityNodeInfo.ACTION_CLICK }) node = node.parent ?: break
            return node
        }
        try {
            show(false)
            val camera = button("Take photo for Jarvis")
            val settings = find("Jarvis settings")
            assertTrue(camera.isEnabled)
            val texts = nodes().mapNotNull { it.text?.toString() }
            assertFalse(texts.contains("Jarvis"))
            assertFalse(texts.any { it.contains("HOST_LABEL") || it.startsWith("Closing this overlay") || it == "Keep call in background" })
            assertTrue(texts.contains("A caption stays readable"))
            val left = Rect().also { settings.getBoundsInScreen(it) }
            val right = Rect().also { camera.getBoundsInScreen(it) }
            assertTrue(left.centerX() < right.centerX())
            assertEquals(left.centerY(), right.centerY())
            assertTrue(camera.performAction(AccessibilityNodeInfo.ACTION_CLICK))
            val deadline = System.currentTimeMillis() + 3_000
            while (action.get() == null && System.currentTimeMillis() < deadline) Thread.sleep(20)
            assertEquals(AssistantAction.Camera, action.get())
            // Let the camera activity's exit transition finish before evidence
            // capture; accessible controls are published before that animation.
            Thread.sleep(350)
            instrumentation.uiAutomation.takeScreenshot().let { bitmap ->
                java.io.File(context.filesDir, "jarvis-minimal-live-test.png").outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }
                bitmap.recycle()
            }
            show(true)
            val limit = System.currentTimeMillis() + 3_000
            while (button("Take photo for Jarvis").isEnabled && System.currentTimeMillis() < limit) Thread.sleep(20)
            assertFalse(button("Take photo for Jarvis").isEnabled)
        } finally {
            instrumentation.runOnMainSync { screen.setContent { Box(Modifier.fillMaxSize()) } }
        }
    }
}
