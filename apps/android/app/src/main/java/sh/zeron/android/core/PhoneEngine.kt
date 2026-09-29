package sh.zeron.android.core

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.PowerManager
import android.provider.Settings
import kotlinx.coroutines.flow.StateFlow
import sh.zeron.runtime.ExecResult
import sh.zeron.runtime.RuntimeController
import sh.zeron.runtime.RuntimeState
import sh.zeron.runtime.ZeronRuntime

/**
 * The on-device engine (`:runtime`) as the UI sees it: state, lifecycle,
 * logs, one-off guest commands, and the Android permissions it leans on.
 */
class PhoneEngine(private val context: Context) {
    private val runtime: RuntimeController = ZeronRuntime.get(context)

    val state: StateFlow<RuntimeState> get() = runtime.state
    val isSupportedAbi: Boolean get() = runtime.isSupportedAbi

    fun start() = runtime.start()
    fun stop() = runtime.stop()
    suspend fun reset() = runtime.reset()
    fun logTail(lines: Int = 200): String = runtime.logTail(lines)
    suspend fun exec(command: String, asRoot: Boolean = false, timeoutMs: Long = 600_000): ExecResult =
        runtime.exec(command, asRoot, timeoutMs)

    /** Projects on the phone engine default here (docs/android.md § Guest layout). */
    val projectsRoot = "/home/zeron/projects"

    /** `git clone` into the projects root; returns the checkout path. */
    suspend fun clone(url: String): Result<String> {
        val name = Formatters.repoName(url) ?: return Result.failure(IllegalArgumentException("That doesn't look like a git URL."))
        val dest = Formatters.joinPath(projectsRoot, name)
        val cmd = "mkdir -p ${Formatters.shellQuote(projectsRoot)} && git clone --progress ${Formatters.shellQuote(url.trim())} ${Formatters.shellQuote(dest)} 2>&1"
        val result = runtime.exec(cmd, timeoutMs = 15 * 60_000)
        return if (result.exitCode == 0) Result.success(dest) else Result.failure(RuntimeException(result.output.lines().lastOrNull { it.isNotBlank() } ?: "git clone failed"))
    }

    val ignoringBatteryOptimizations: Boolean
        get() = (context.getSystemService(Context.POWER_SERVICE) as PowerManager).isIgnoringBatteryOptimizations(context.packageName)

    /** The system prompt to exempt Zeron from battery optimization (asked when the engine starts). */
    fun batteryIntent(): Intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:${context.packageName}"))

    fun stateLabel(s: RuntimeState): String = when (s) {
        RuntimeState.NotInstalled -> "Not installed"
        is RuntimeState.Bootstrapping -> s.step
        RuntimeState.Starting -> "Starting"
        is RuntimeState.Running -> "Running"
        RuntimeState.Stopped -> "Stopped"
        is RuntimeState.Failed -> "Failed"
    }
}
