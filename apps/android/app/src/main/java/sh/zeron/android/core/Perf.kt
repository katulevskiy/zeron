package sh.zeron.android.core

import android.os.SystemClock
import android.util.Log
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.withFrameNanos

/**
 * Interaction timing for `adb logcat -s ZeronPerf` (scripts/android/measure-tab-switch.sh). [begin] stamps the
 * moment an interaction is asked for; [PerfFrame] reports when the frame that painted its result has been handed
 * off, i.e. the start of the next vsync after the composition that followed it. A main thread that is busy for
 * 200 ms between the two shows up as ~200 ms; an instant switch is one or two frames (16-33 ms at 60 Hz).
 */
object Perf {
    const val TAG = "ZeronPerf"

    @Volatile private var label = ""
    @Volatile private var t0 = 0L
    private var cpu0 = 0L

    /** Debug builds: sample the main thread's stack while an interaction is timed and log where the time went. */
    @Volatile var sampling = false
    private var sampler: Thread? = null
    private val inclusive = HashMap<String, Int>()
    private val self = HashMap<String, Int>()
    private var samples = 0

    fun begin(what: String) {
        label = what
        t0 = SystemClock.elapsedRealtimeNanos()
        cpu0 = android.os.Debug.threadCpuTimeNanos()
        if (sampling) startSampler()
    }

    private fun startSampler() {
        val main = android.os.Looper.getMainLooper().thread
        synchronized(inclusive) { inclusive.clear(); self.clear(); samples = 0 }
        val thread = Thread {
            while (!Thread.currentThread().isInterrupted) {
                val stack = main.stackTrace
                if (stack.isNotEmpty()) synchronized(inclusive) {
                    samples++
                    val seen = HashSet<String>()
                    for (f in stack) {
                        val key = f.className.substringAfterLast('.') + "." + f.methodName
                        if (seen.add(key)) inclusive.merge(key, 1, Int::plus)
                    }
                    val top = stack[0]
                    self.merge(top.className.substringAfterLast('.') + "." + top.methodName, 1, Int::plus)
                }
                try { Thread.sleep(1) } catch (_: InterruptedException) { return@Thread }
            }
        }
        thread.name = "ZeronPerfSampler"
        thread.isDaemon = true
        sampler = thread
        thread.start()
    }

    private fun report() {
        sampler?.interrupt()
        sampler = null
        synchronized(inclusive) {
            Log.d(TAG, "samples=$samples (~1 ms apart; compose frames dominate)")
            inclusive.entries.sortedByDescending { it.value }.take(400).forEach { Log.d(TAG, "incl %4d %s".format(it.value, it.key)) }
            self.entries.sortedByDescending { it.value }.take(15).forEach { Log.d(TAG, "self %4d %s".format(it.value, it.key)) }
        }
    }

    /** Milliseconds since [begin], or -1 when nothing is being timed. */
    fun elapsedMs(): Double = if (label.isEmpty()) -1.0 else (SystemClock.elapsedRealtimeNanos() - t0) / 1e6

    fun mark(step: String) {
        if (label.isEmpty()) return
        Log.d(TAG, "%s: %s +%.1f ms".format(label, step, elapsedMs()))
    }

    fun finish(step: String) {
        if (label.isEmpty()) return
        Log.d(TAG, "%s: %s +%.1f ms wall, %.1f ms main-thread cpu (done)".format(label, step, elapsedMs(), (android.os.Debug.threadCpuTimeNanos() - cpu0) / 1e6))
        label = ""
        if (sampling) report()
    }
}

/** Reports the first frame after [key] changed (and was composed) to [Perf]. */
@Composable
fun PerfFrame(key: Any?) {
    LaunchedEffect(key) {
        if (Perf.elapsedMs() < 0) return@LaunchedEffect
        Perf.mark("composed")
        withFrameNanos { }
        Perf.finish("first frame after composition")
    }
}
