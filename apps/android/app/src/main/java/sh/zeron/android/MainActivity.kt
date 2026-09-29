package sh.zeron.android

import android.content.Intent
import android.graphics.Color
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import sh.zeron.android.core.AppMode
import sh.zeron.android.design.Appearance
import sh.zeron.android.design.Overlays
import sh.zeron.android.design.ZeronTheme
import sh.zeron.android.ui.AppRoot

class MainActivity : ComponentActivity() {
    private val model get() = (application as ZeronApp).model

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        applyLaunchOptions(intent)
        enableEdgeToEdge()
        setContent {
            val appearance by model.appearance.collectAsState()
            val dark = when (appearance) {
                Appearance.System -> isSystemInDarkTheme()
                Appearance.Light -> false
                Appearance.Dark -> true
            }
            LaunchedEffect(dark) {
                // Transparent bars; icons follow the app's appearance, not the system's.
                val style = if (dark) SystemBarStyle.dark(Color.TRANSPARENT) else SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT)
                enableEdgeToEdge(statusBarStyle = style, navigationBarStyle = style)
            }
            val overlays = remember { Overlays() }
            ZeronTheme(appearance, overlays) { AppRoot(model, this) }
        }
        handleIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        applyLaunchOptions(intent)
        handleIntent(intent)
    }

    private fun handleIntent(intent: Intent?) {
        intent ?: return
        val data = intent.data
        if (data?.scheme == "zeron") {
            // WorkOS AuthKit redirect (zeron://callback?code=…&state=…).
            model.handleAuthCallback(data.toString())
            return
        }
        intent.getStringExtra(EXTRA_CHAT)?.let { model.openRequests.tryEmit(it) }
    }

    /**
     * Development switches (debug builds; Demo only):
     * `adb shell am start -n sh.zeron.android/.MainActivity --es mode demo --es demo huge --ez fast true`
     */
    private fun applyLaunchOptions(intent: Intent?) {
        if (!BuildConfig.DEBUG || intent == null) return
        var restart = false
        intent.getStringExtra("demo")?.let {
            model.prefs.demoScale = it
            restart = true
        }
        if (intent.hasExtra("fast")) {
            model.prefs.demoFast = intent.getBooleanExtra("fast", false)
            restart = true
        }
        intent.getStringExtra("appearance")?.let { a ->
            runCatching { Appearance.valueOf(a.replaceFirstChar { it.uppercase() }) }.getOrNull()?.let {
                model.prefs.appearance = it
                model.appearance.value = it
            }
        }
        when (intent.getStringExtra("mode")) {
            "demo" -> model.chooseMode(AppMode.Demo)
            "reset" -> model.resetMode()
            null -> if (restart) model.restartDemo()
        }
    }

    companion object {
        const val EXTRA_CHAT = "chat"
    }
}
