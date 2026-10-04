package sh.zeron.android.voice.assistant

import android.os.Build
import android.view.View
import android.view.Window
import android.view.WindowManager
import androidx.annotation.RequiresApi
import java.util.function.Consumer
import kotlin.math.roundToInt

/** Cross-window blur is performed by Android's compositor: neither assistant
 * text nor a captured copy of another app is blurred. Older/unsupported devices
 * retain the translucent Compose surface, with a stronger tint for readability. */
internal class JarvisAssistantGlass(
    private val window: Window,
    private val onBlurChanged: (Boolean) -> Unit,
) : View.OnAttachStateChangeListener {
    private val manager = window.context.getSystemService(WindowManager::class.java)
    private var listener: Consumer<Boolean>? = null
    private var closed = false

    init {
        onBlurChanged(false)
        window.decorView.addOnAttachStateChangeListener(this)
        if (window.decorView.isAttachedToWindow) onViewAttachedToWindow(window.decorView)
    }

    override fun onViewAttachedToWindow(view: View) {
        if (!closed && Build.VERSION.SDK_INT >= 31 && listener == null) registerBlurListener()
    }

    @RequiresApi(31)
    private fun registerBlurListener() {
        listener = Consumer<Boolean> { enabled ->
            if (!closed) {
                updateWindowBlur(enabled)
                onBlurChanged(enabled)
            }
        }.also { manager.addCrossWindowBlurEnabledListener(it) }
    }

    override fun onViewDetachedFromWindow(view: View) {
        if (Build.VERSION.SDK_INT >= 31) {
            unregisterBlurListener()
            updateWindowBlur(false)
        }
        onBlurChanged(false)
    }

    @RequiresApi(31)
    private fun updateWindowBlur(enabled: Boolean) {
        window.attributes = window.attributes.apply {
            blurBehindRadius = if (enabled) (24 * window.context.resources.displayMetrics.density).roundToInt() else 0
            flags = if (enabled) flags or WindowManager.LayoutParams.FLAG_BLUR_BEHIND
                else flags and WindowManager.LayoutParams.FLAG_BLUR_BEHIND.inv()
        }
    }

    @RequiresApi(31)
    private fun unregisterBlurListener() {
        listener?.let(manager::removeCrossWindowBlurEnabledListener)
        listener = null
    }

    fun close() {
        closed = true
        window.decorView.removeOnAttachStateChangeListener(this)
        onViewDetachedFromWindow(window.decorView)
    }
}
