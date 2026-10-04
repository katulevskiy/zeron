package sh.zeron.android.voice.screen

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.app.KeyguardManager
import android.graphics.Bitmap
import android.graphics.Path
import android.graphics.Rect
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
import kotlinx.coroutines.suspendCancellableCoroutine
import sh.zeron.android.ZeronApplication
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/** Bound only after the user explicitly enables it in Android Accessibility. */
class JarvisAccessibilityService : AccessibilityService() {
    override fun onServiceConnected() { current = this }
    override fun onAccessibilityEvent(event: AccessibilityEvent?) { }
    override fun onInterrupt() { (application as ZeronApplication).existingModel?.jarvis?.screen?.stopControl() }
    override fun onDestroy() {
        if (current === this) current = null
        (application as ZeronApplication).existingModel?.jarvis?.screen?.stopControl()
        super.onDestroy()
    }
    private fun target(): AccessibilityWindowInfo {
        check(!getSystemService(KeyguardManager::class.java).isKeyguardLocked) { "Unlock your phone" }
        val target = windows.filter {
            it.type == AccessibilityWindowInfo.TYPE_APPLICATION ||
                (it.type == AccessibilityWindowInfo.TYPE_SYSTEM && (it.isActive || it.isFocused))
        }.maxByOrNull { it.layer }
            ?: error("Open another app to inspect its screen")
        check(target.root?.packageName?.toString() != packageName) { "Open another app to inspect or control its screen" }
        return target
    }
    private fun label(node: AccessibilityNodeInfo, depth: Int = 0): String {
        if (node.isPassword) return "Password field"
        val own = listOf(node.contentDescription, node.text, node.hintText).mapNotNull { it?.toString()?.takeIf(String::isNotBlank) }.distinct().joinToString(" · ")
        if (own.isNotEmpty() || depth >= 2) return own.take(240)
        return (0 until minOf(node.childCount, 12)).mapNotNull { node.getChild(it) }.joinToString(" ") { label(it, depth + 1) }.take(240)
    }
    internal fun observe(): ScreenSnapshot {
        // The platform can retain stale node text even after an action has
        // visibly succeeded. Flush once per snapshot, keeping prefetch/cache
        // available within that read; older Android refreshes each node.
        if (Build.VERSION.SDK_INT >= 33) clearCache()
        val window = target()
        val root = window.root ?: error("This app exposes no accessible controls")
        val metrics = android.util.DisplayMetrics()
        @Suppress("DEPRECATION")
        getSystemService(WindowManager::class.java).defaultDisplay.getRealMetrics(metrics)
        @Suppress("DEPRECATION")
        val rotation = getSystemService(WindowManager::class.java).defaultDisplay.rotation
        val elements = mutableListOf<ScreenElement>()
        val visibleText = linkedSetOf<String>()
        var visited = 0
        fun visit(node: AccessibilityNodeInfo, path: List<Int>, depth: Int) {
            if (++visited > 600 || depth > 30) return
            if (Build.VERSION.SDK_INT < 33 && !node.refresh()) return
            if (!node.isVisibleToUser) return
            if (!node.isPassword) node.text?.toString()?.take(240)?.takeIf(String::isNotBlank)?.let(visibleText::add)
            val bounds = Rect(); node.getBoundsInScreen(bounds)
            if (node.isEnabled && !node.isPassword && !bounds.isEmpty && elements.size < 180) {
                val ops = mutableListOf<String>()
                val supported = node.actionList.map { it.id }.toSet()
                if (node.isClickable && AccessibilityNodeInfo.ACTION_CLICK in supported) ops.add("CLICK")
                if (node.isEditable && AccessibilityNodeInfo.ACTION_SET_TEXT in supported) ops.add("TYPE")
                if (AccessibilityNodeInfo.ACTION_SCROLL_FORWARD in supported) ops.add("SCROLL_FORWARD")
                if (AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD in supported) ops.add("SCROLL_BACKWARD")
                if (ops.isNotEmpty()) elements.add(ScreenElement(path.joinToString("_").ifEmpty { "root" }, label(node),
                    node.className?.toString().orEmpty(), listOf(bounds.left, bounds.top, bounds.right, bounds.bottom), ops,
                    if (node.isEditable) node.text?.toString().orEmpty().take(240) else "", node.isChecked))
            }
            for (i in 0 until minOf(node.childCount, 200)) node.getChild(i)?.let { visit(it, path + i, depth + 1) }
        }
        visit(root, emptyList(), 0)
        return ScreenSnapshot(root.packageName?.toString().orEmpty(), window.id, metrics.widthPixels, metrics.heightPixels,
            rotation, visibleText.joinToString("\n").take(6000), elements, SystemClock.elapsedRealtime())
    }
    internal suspend fun execute(observed: ScreenSnapshot, decision: ScreenDecision, text: String?) {
        val current = observe()
        ScreenPolicy.requireFresh(observed, current, SystemClock.elapsedRealtime())
        check(decision.operation !in listOf("DONE", "BLOCKED", "WAIT"))
        if (decision.operation in listOf("BACK", "HOME")) {
            check(performGlobalAction(if (decision.operation == "BACK") GLOBAL_ACTION_BACK else GLOBAL_ACTION_HOME)) { "Navigation unavailable" }
            return
        }
        val element = current.elements.firstOrNull { it.id == decision.target } ?: error("Target disappeared")
        check(decision.operation in element.operations)
        val targetWindow = target()
        if (targetWindow.id != current.windowId || targetWindow.root?.packageName?.toString() != current.packageName) throw StaleScreenException()
        var node = targetWindow.root ?: error("Target disappeared")
        if (element.id != "root") for (index in element.id.split('_')) node = node.getChild(index.toInt()) ?: error("Target disappeared")
        check(node.refresh() && node.isVisibleToUser && node.isEnabled && !node.isPassword)
        val bounds = Rect(); node.getBoundsInScreen(bounds)
        check(listOf(bounds.left, bounds.top, bounds.right, bounds.bottom) == element.bounds) { "Target moved" }
        check(label(node) == element.label && node.className?.toString().orEmpty() == element.role && node.isChecked == element.checked) { "Target changed" }
        if (node.isEditable) check(node.text?.toString().orEmpty().take(240) == element.value) { "Field changed" }
        val performed = when (decision.operation) {
            "CLICK" -> node.performAction(AccessibilityNodeInfo.ACTION_CLICK)
            "TYPE" -> {
                require(!text.isNullOrEmpty() && text.length <= 2000)
                node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply { putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text) })
            }
            "SCROLL_FORWARD" -> node.performAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD)
            "SCROLL_BACKWARD" -> node.performAction(AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD)
            else -> error("Unsupported action")
        }
        // Gesture fallback only after a fresh validation, never inferred coordinates.
        if (!performed && decision.operation == "CLICK") tap(current, bounds.exactCenterX().toDouble(), bounds.exactCenterY().toDouble())
        else check(performed) { "App rejected the action" }
    }
    internal suspend fun tap(observed: ScreenSnapshot, x: Double, y: Double) {
        ScreenPolicy.requireFresh(observed, observe(), SystemClock.elapsedRealtime())
        ScreenPolicy.requirePoint(x, y, observed)
        gesture(Path().apply { moveTo(x.toFloat(), y.toFloat()) }, 60)
    }
    internal suspend fun swipe(observed: ScreenSnapshot, x: Double, y: Double, endX: Double, endY: Double) {
        ScreenPolicy.requireFresh(observed, observe(), SystemClock.elapsedRealtime())
        ScreenPolicy.requirePoint(x, y, observed); ScreenPolicy.requirePoint(endX, endY, observed)
        gesture(Path().apply { moveTo(x.toFloat(), y.toFloat()); lineTo(endX.toFloat(), endY.toFloat()) }, 250)
    }
    private suspend fun gesture(path: Path, duration: Long): Unit = suspendCancellableCoroutine { continuation ->
        val accepted = dispatchGesture(GestureDescription.Builder().addStroke(GestureDescription.StrokeDescription(path, 0, duration)).build(),
            object : GestureResultCallback() {
                override fun onCompleted(gestureDescription: GestureDescription?) { if (continuation.isActive) continuation.resume(Unit) }
                override fun onCancelled(gestureDescription: GestureDescription?) { if (continuation.isActive) continuation.resumeWithException(IllegalStateException("Gesture cancelled")) }
            }, null)
        if (!accepted && continuation.isActive) continuation.resumeWithException(IllegalStateException("Gesture unavailable"))
    }
    internal fun imageBounds(): List<Int> {
        if (Build.VERSION.SDK_INT >= 34) { val r = Rect(); target().getBoundsInScreen(r); return listOf(r.left, r.top, r.right, r.bottom) }
        val s = observe(); return listOf(0, 0, s.width, s.height)
    }
    internal suspend fun screenshot(): Bitmap = suspendCancellableCoroutine { continuation ->
        if (Build.VERSION.SDK_INT < 30) {
            continuation.resumeWithException(IllegalStateException("Use live screen sharing on Android 10")); return@suspendCancellableCoroutine
        }
        val callback = object : TakeScreenshotCallback {
            override fun onSuccess(result: ScreenshotResult) {
                val bitmap = try { Bitmap.wrapHardwareBuffer(result.hardwareBuffer, result.colorSpace)?.let { wrapped ->
                    wrapped.copy(Bitmap.Config.ARGB_8888, false).also { wrapped.recycle() }
                } } finally { result.hardwareBuffer.close() }
                if (bitmap == null) { if (continuation.isActive) continuation.resumeWithException(IllegalStateException("Screenshot unavailable")) }
                else if (continuation.isActive) continuation.resume(bitmap) { _, value, _ -> value.recycle() }
                else bitmap.recycle()
            }
            override fun onFailure(errorCode: Int) {
                if (continuation.isActive) continuation.resumeWithException(IllegalStateException("Screenshot unavailable (Android $errorCode). Protected screens cannot be captured."))
            }
        }
        if (Build.VERSION.SDK_INT >= 34) takeScreenshotOfWindow(target().id, mainExecutor, callback)
        else takeScreenshot(android.view.Display.DEFAULT_DISPLAY, mainExecutor, callback)
    }
    companion object { internal var current: JarvisAccessibilityService? = null; private set }
}
