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

    /**
     * Restart the engine process (bootstrap is kept), e.g. so it adopts the
     * account it just signed in to or out of: its workspace is fixed per run.
     * Starts it when it isn't running.
     */
    fun restart()

    /**
     * A development edge the engine joins instead of Zeron's (`zeron
     * local-edge` on another machine), or null for the default: the signed-in
     * account, else local-only. Takes effect on the next (re)start.
     */
    var customServer: CustomServer?

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
    /** The engine answers on its IPC port; ask it for its edge and identity. */
    data class Running(
        val ipcPort: Int,
        val ipcToken: String,
        val deviceName: String,
    ) : RuntimeState {
        val ipcUrl: String get() = "ws://127.0.0.1:$ipcPort"
    }
    data object Stopped : RuntimeState
    data class Failed(val reason: String, val logTail: String) : RuntimeState
}

data class ExecResult(val exitCode: Int, val output: String)

/** A development edge: its URL (e.g. `http://10.0.2.2:27700`) and shared secret. */
data class CustomServer(val edgeUrl: String, val token: String)
