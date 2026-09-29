package sh.zeron.runtime

import android.content.Context
import kotlinx.coroutines.flow.StateFlow

/** Entry point to the on-device engine runtime (docs/android.md § Runtime API). */
object ZeronRuntime {
    @Volatile private var instance: GuestRuntime? = null

    fun get(context: Context): RuntimeController =
        instance ?: synchronized(this) {
            instance ?: GuestRuntime(context.applicationContext).also { instance = it }
        }

    internal fun impl(context: Context): GuestRuntime = get(context) as GuestRuntime
}

interface RuntimeController {
    val state: StateFlow<RuntimeState>
    val isSupportedAbi: Boolean

    /** Bootstrap if needed, then run the engine under [RuntimeService]. */
    fun start()
    fun stop()

    /** Wipe the guest (keeps nothing, secrets included). */
    suspend fun reset()
    fun logTail(lines: Int = 200): String

    /** One-off `sh -lc` in the guest with the engine's environment. */
    suspend fun exec(
        command: String,
        asRoot: Boolean = false,
        timeoutMs: Long = 600_000,
    ): ExecResult
}

sealed interface RuntimeState {
    data object NotInstalled : RuntimeState
    data class Bootstrapping(val step: String, val progress: Float?) : RuntimeState
    data object Starting : RuntimeState
    data class Running(
        val edgeUrl: String,
        val edgeToken: String,
        val ipcPort: Int,
        val ipcToken: String,
        val deviceName: String,
    ) : RuntimeState
    data object Stopped : RuntimeState
    data class Failed(val reason: String, val logTail: String) : RuntimeState
}

data class ExecResult(val exitCode: Int, val output: String)
