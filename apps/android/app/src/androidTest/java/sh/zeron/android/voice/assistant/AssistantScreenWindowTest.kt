package sh.zeron.android.voice.assistant

import android.content.Intent
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import sh.zeron.android.MainActivity
import sh.zeron.android.ZeronApplication
import sh.zeron.android.voice.*
import uniffi.zeron_core.*

@RunWith(AndroidJUnit4::class)
class AssistantScreenWindowTest {
    @Test fun actualScreenButtonUsesAssistantWindowAndBackPreservesCall() = runBlocking<Unit> {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val automation = instrumentation.uiAutomation
        automation.serviceInfo = automation.serviceInfo.apply { flags = flags or android.accessibilityservice.AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS }
        fun shell(command: String) = android.os.ParcelFileDescriptor.AutoCloseInputStream(automation.executeShellCommand(command)).use { String(it.readBytes()) }
        val previousService = shell("settings get secure voice_interaction_service").trim()
        val previousAssistant = shell("settings get secure assistant").trim()
        val model = (context.applicationContext as ZeronApplication).model
        withContext(Dispatchers.Main) {
            context.startActivity(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK).putExtra("demo", true))
            model.startDemo()
        }
        withTimeout(10_000) { while (model.client.value == null) delay(50) }
        val controller = model.jarvis
        val screen = controller.screen
        val oldContext = screen.settings.contextEnabled.value
        val oldControl = screen.settings.controlEnabled.value
        var session: JarvisAssistantSession? = null
        fun nodes(): List<AccessibilityNodeInfo> {
            if (android.os.Build.VERSION.SDK_INT >= 33) automation.clearCache()
            val result = mutableListOf<AccessibilityNodeInfo>()
            fun visit(node: AccessibilityNodeInfo) { result.add(node); for (i in 0 until node.childCount) node.getChild(i)?.let(::visit) }
            automation.windows.mapNotNull { it.root }.forEach(::visit)
            if (result.isEmpty()) automation.rootInActiveWindow?.let(::visit)
            return result
        }
        suspend fun click(description: String) {
            android.util.Log.i("JarvisScreenValidation", "assistant screen test click=$description")
            withTimeout(5000) {
                var node: AccessibilityNodeInfo? = null
                while (node == null) { node = nodes().firstOrNull { it.contentDescription?.toString() == description || it.text?.toString() == description }; if (node == null) delay(50) }
                var target = node!!
                while (!target.isClickable && target.parent != null) target = target.parent
                assertTrue("Click $description", target.performAction(AccessibilityNodeInfo.ACTION_CLICK))
            }
        }
        try {
            shell("settings put secure voice_interaction_service sh.zeron.android/.voice.assistant.JarvisAssistantService")
            shell("settings put secure assistant sh.zeron.android/.voice.assistant.JarvisAssistantService")
            shell("am start -a android.settings.SETTINGS")
            shell("input keyevent --longpress 26")
            val current = JarvisAssistantSession::class.java.getDeclaredField("current").apply { isAccessible = true }
            withTimeout(10_000) { while (session == null) {
                withContext(Dispatchers.Main) { session = (current.get(null) as? java.lang.ref.WeakReference<*>)?.get() as? JarvisAssistantSession }
                if (session == null) delay(50)
            } }
            val assistant = checkNotNull(session)
            delay(500)
            withContext(Dispatchers.Main) {
                (JarvisAssistantSession::class.java.getDeclaredField("startJob").apply { isAccessible = true }.get(assistant) as? Job)?.cancel()
                controller.stop(); screen.setContext(true); screen.setControl(true)
                val generation = JarvisController::class.java.getDeclaredField("generation").apply { isAccessible = true }.get(controller) as VoiceGeneration
                generation.begin()
                @Suppress("UNCHECKED_CAST")
                val state = JarvisController::class.java.getDeclaredField("_state").apply { isAccessible = true }.get(controller) as MutableStateFlow<JarvisState>
                state.value = JarvisState(live = true, call = VoiceCallState(VoiceCallPhase.ACTIVE, VoiceOrb.LISTENING, null,
                    VoiceCallWork.IDLE, false, false, "", null, null, 0f, 0f, emptyList()))
            }
            click("Screen context and phone control")
            delay(200)
            withContext(Dispatchers.Main) { assertTrue(assistant.screenControls.value); assertTrue(controller.state.value.live) }
            withTimeout(5000) { while (nodes().none { it.text?.toString() == "Phone screen" }) delay(50) }
            withContext(Dispatchers.Main) {
                assertEquals(2031 /* Android's TYPE_VOICE_INTERACTION */, assistant.window.window!!.attributes.type)
                assertTrue(controller.state.value.live)
                assistant.onBackPressed()
            }
            delay(200)
            assertTrue("Back closes controls without ending voice", controller.state.value.live)
            click("Screen context and phone control")
            // Missing accessibility must produce a message, not a fatal exception.
            withContext(Dispatchers.Main) { screen.acceptAssist(null) }
            click("Share screenshot")
            withTimeout(5000) { while (screen.state.value.message?.contains("Android Accessibility") != true) delay(50) }
            assertTrue(controller.state.value.live)
            click("Start live screen sharing")
            withTimeout(5000) { while (nodes().none { it.text?.toString() in listOf("A single app", "Entire screen", "Start now", "Start") }) delay(50) }
            assertTrue("Consent handoff preserves voice", controller.state.value.live)
            shell("input keyevent 4")
            android.util.Log.i("JarvisScreenValidation", "assistant_screen_button=true owned_window=true back_preserves_call=true missing_accessibility_graceful=true capture_consent=true")
        } catch (e: Exception) {
            automation.takeScreenshot()?.let { bitmap ->
                java.io.File(context.filesDir, "jarvis-screen-window-failure.png").outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }
                bitmap.recycle()
            }
            android.util.Log.i("JarvisScreenValidation", "screen test visible labels=${nodes().mapNotNull { it.text?.toString() }.take(30)}")
            throw e
        } finally {
            withContext(Dispatchers.Main) { session?.dismiss(); screen.setContext(oldContext); screen.setControl(oldControl); controller.stop() }
            shell("settings put secure voice_interaction_service '${previousService.takeUnless { it == "null" }.orEmpty()}'")
            shell("settings put secure assistant '${previousAssistant.takeUnless { it == "null" }.orEmpty()}'")
        }
    }
}
