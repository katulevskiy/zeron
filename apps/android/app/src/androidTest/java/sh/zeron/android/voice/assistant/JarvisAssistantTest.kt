package sh.zeron.android.voice.assistant

import android.Manifest
import android.content.ComponentName
import android.content.Intent
import android.content.pm.PackageManager
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import sh.zeron.android.MainActivity
import sh.zeron.android.ZeronApplication

@RunWith(AndroidJUnit4::class)
class JarvisAssistantTest {
    @Test fun systemEntryPointsAreProtectedAndDoNotRequestScreenContext() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val pm = context.packageManager
        val registration = pm.getServiceInfo(ComponentName(context, JarvisAssistantService::class.java), PackageManager.GET_META_DATA)
        assertTrue(registration.exported)
        assertEquals(Manifest.permission.BIND_VOICE_INTERACTION, registration.permission)
        val session = pm.getServiceInfo(ComponentName(context, JarvisAssistantSessionService::class.java), 0)
        assertTrue(session.exported)
        assertEquals(Manifest.permission.BIND_VOICE_INTERACTION, session.permission)
        val recognition = pm.getServiceInfo(ComponentName(context, JarvisRecognitionService::class.java), 0)
        assertEquals(Manifest.permission.RECORD_AUDIO, recognition.permission)
        assertFalse(pm.getActivityInfo(ComponentName(context, JarvisAssistantPermissionActivity::class.java), 0).exported)
        val parser = registration.loadXmlMetaData(pm, "android.voice_interaction")
        parser.use {
            while (it.next() != org.xmlpull.v1.XmlPullParser.START_TAG) { }
            val android = "http://schemas.android.com/apk/res/android"
            assertEquals("voice-interaction-service", it.name)
            assertTrue(it.getAttributeBooleanValue(android, "supportsAssist", false))
            assertFalse(it.getAttributeBooleanValue(android, "supportsLaunchVoiceAssistFromKeyguard", true))
            assertEquals(JarvisAssistantSessionService::class.java.name, it.getAttributeValue(android, "sessionService"))
            assertEquals(JarvisRecognitionService::class.java.name, it.getAttributeValue(android, "recognitionService"))
        }
        val permissions = pm.getPackageInfo(context.packageName, PackageManager.GET_PERMISSIONS).requestedPermissions.orEmpty()
        assertFalse(permissions.contains(Manifest.permission.SYSTEM_ALERT_WINDOW))
        assertTrue(permissions.contains(Manifest.permission.CAMERA))
    }

    @Test fun cameraOpensInTheRealAssistantWindowOverAnotherAppAndBackClosesOnlyCamera() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        fun shell(command: String) = android.os.ParcelFileDescriptor.AutoCloseInputStream(instrumentation.uiAutomation.executeShellCommand(command)).use { it.readBytes() }
        // A standalone invocation of this test needs the same initialized
        // demo runtime that earlier suite tests provide. A cold client bind
        // would otherwise close this deliberately call-free camera fixture.
        instrumentation.runOnMainSync {
            context.startActivity(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK).putExtra("demo", true))
        }
        val bootedBy = System.currentTimeMillis() + 10_000
        var booted = false
        while (!booted && System.currentTimeMillis() < bootedBy) {
            instrumentation.runOnMainSync { booted = (context.applicationContext as ZeronApplication).model.client.value != null }
            if (!booted) Thread.sleep(50)
        }
        assertTrue(booted)
        instrumentation.uiAutomation.grantRuntimePermission(context.packageName, Manifest.permission.CAMERA)
        shell("settings put secure voice_interaction_service sh.zeron.android/.voice.assistant.JarvisAssistantService")
        shell("settings put secure assistant sh.zeron.android/.voice.assistant.JarvisAssistantService")
        shell("am start -a android.settings.SETTINGS")
        shell("input keyevent --longpress 26")
        val field = JarvisAssistantSession::class.java.getDeclaredField("current").apply { isAccessible = true }
        var session: JarvisAssistantSession? = null
        val until = System.currentTimeMillis() + 10_000
        while (session == null && System.currentTimeMillis() < until) {
            instrumentation.runOnMainSync { session = (field.get(null) as? java.lang.ref.WeakReference<*>)?.get() as? JarvisAssistantSession }
            if (session == null) Thread.sleep(50)
        }
        val assistant = checkNotNull(session)
        Thread.sleep(500)
        try {
            instrumentation.runOnMainSync { assistant.camera.open() }
            val readyBy = System.currentTimeMillis() + 15_000
            while (!assistant.camera.state.value.ready && System.currentTimeMillis() < readyBy) Thread.sleep(50)
            assertTrue("Camera must stream inside TYPE_VOICE_INTERACTION while Settings is the underlying app", assistant.camera.state.value.ready)
            Thread.sleep(400)
            instrumentation.uiAutomation.takeScreenshot().let { bitmap ->
                java.io.File(context.filesDir, "jarvis-system-circle-test.png").outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }
                bitmap.recycle()
            }
            instrumentation.runOnMainSync { assistant.onBackPressed() }
            assertFalse(assistant.camera.state.value.open)
            assertTrue(assistant.lifecycle.currentState.isAtLeast(androidx.lifecycle.Lifecycle.State.RESUMED))
        } finally { instrumentation.runOnMainSync { assistant.dismiss() } }
    }

    @Test fun staleSessionCannotCancelNewOverlayAndBackgroundRequestsCannotStart() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
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
        assertTrue(resumed)
        instrumentation.runOnMainSync {
            val model = (context.applicationContext as ZeronApplication).model
            val controller = model.jarvis
            model.onBackground()
            controller.stop()
            model.pendingRoute.value = null
            val before = controller.state.value
            controller.requestStart()
            assertEquals(before, controller.state.value)
            assertNull(model.pendingRoute.value)
            val old = Any()
            val new = Any()
            controller.showAssistant(old)
            controller.showAssistant(new)
            controller.hideAssistant(old, false)
            assertTrue(controller.assistantVisible)
            assertTrue(controller.mediaVisible)
            controller.requestStartFromAssistant()
            // Demo has no voice-capable host. The error stays in the overlay
            // and must not navigate the underlying activity to settings.
            assertNotNull(controller.state.value.error)
            assertNull(model.pendingRoute.value)
            controller.hideAssistant(new, false)
            assertFalse(controller.assistantVisible)
            assertFalse(controller.mediaVisible)
            controller.requestStartFromAssistant()
            assertFalse(controller.state.value.live)
            model.onForeground()
        }
    }
}
