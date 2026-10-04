package sh.zeron.android.voice.assistant

import android.content.Intent
import android.os.Build
import android.provider.Settings
import android.view.WindowManager
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import sh.zeron.android.MainActivity
import java.util.concurrent.atomic.AtomicReference

@RunWith(AndroidJUnit4::class)
class JarvisAssistantGlassTest {
    /** Real platform callbacks/window attributes, including a live global blur
     * switch. Unsupported GPUs must stay on the same usable fallback path. */
    @Test fun systemBlurSwitchUpdatesWindowAndListenerIsReleased() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        fun shell(command: String) = instrumentation.uiAutomation.executeShellCommand(command).use {
            java.io.FileInputStream(it.fileDescriptor).readBytes()
        }
        shell("input keyevent 4")
        instrumentation.runOnMainSync {
            context.startActivity(Intent(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK).putExtra("demo", true))
        }
        var activity: MainActivity? = null
        val deadline = System.currentTimeMillis() + 10_000
        while (activity == null && System.currentTimeMillis() < deadline) {
            instrumentation.runOnMainSync {
                activity = ActivityLifecycleMonitorRegistry.getInstance()
                    .getActivitiesInStage(Stage.RESUMED).filterIsInstance<MainActivity>().firstOrNull()
            }
            if (activity == null) Thread.sleep(50)
        }
        val screen = checkNotNull(activity)
        val observed = AtomicReference<Boolean?>(null)
        var glass: JarvisAssistantGlass? = null
        val original = Settings.Global.getString(context.contentResolver, "disable_window_blurs")
        fun awaitBlur(expected: Boolean) {
            val until = System.currentTimeMillis() + 5_000
            while (observed.get() != expected && System.currentTimeMillis() < until) Thread.sleep(20)
            assertEquals(expected, observed.get())
            instrumentation.runOnMainSync {
                val attrs = screen.window.attributes
                assertEquals(expected, attrs.flags and WindowManager.LayoutParams.FLAG_BLUR_BEHIND != 0)
                if (Build.VERSION.SDK_INT >= 31) {
                    if (expected) assertTrue(attrs.blurBehindRadius > 0) else assertEquals(0, attrs.blurBehindRadius)
                }
            }
        }
        try {
            shell("settings put global disable_window_blurs 0")
            instrumentation.runOnMainSync { glass = JarvisAssistantGlass(screen.window, observed::set) }
            val manager = context.getSystemService(WindowManager::class.java)
            val enabled = Build.VERSION.SDK_INT >= 31 && manager.isCrossWindowBlurEnabled
            awaitBlur(enabled)
            shell("settings put global disable_window_blurs 1")
            awaitBlur(false)
            shell("settings put global disable_window_blurs 0")
            awaitBlur(enabled)
            instrumentation.runOnMainSync { glass!!.close() }
            assertEquals(false, observed.get())
            observed.set(null)
            shell("settings put global disable_window_blurs 1")
            Thread.sleep(100)
            assertNull("Closed glass must no longer receive system callbacks", observed.get())
        } finally {
            instrumentation.runOnMainSync { glass?.close() }
            if (original == null) shell("settings delete global disable_window_blurs")
            else shell("settings put global disable_window_blurs $original")
        }
    }
}
