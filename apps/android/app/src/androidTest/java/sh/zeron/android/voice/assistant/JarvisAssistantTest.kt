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
    }

    @Test fun staleSessionCannotCancelNewOverlayAndBackgroundRequestsCannotStart() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        instrumentation.runOnMainSync {
            context.startActivity(Intent(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK).putExtra("demo", true))
        }
        instrumentation.waitForIdleSync()
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
