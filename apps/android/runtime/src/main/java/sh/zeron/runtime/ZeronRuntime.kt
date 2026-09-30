package sh.zeron.runtime

import android.content.Context
import kotlinx.coroutines.flow.StateFlow
import java.io.File

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

    /**
     * Host directory holding the guest's `/` (app-private). Guest paths the
     * engine reports map onto it, except `/tmp`, which is [guestTmpDir].
     */
    val guestRootDir: File

    /** Host directory bound at the guest's `/tmp` (and `/dev/shm`). */
    val guestTmpDir: File

    /**
     * Developer override: join a shared edge (`zeron local-edge`) instead of
     * embedding one. `null` = the embedded loopback edge.
     */
    val customEdge: CustomEdge?

    /** Persist the override and restart a running engine onto it. */
    fun setCustomEdge(edge: CustomEdge?)
}

/**
 * A shared edge the engine joins with `ZERON_LOCAL_EDGE_URL` +
 * `ZERON_LOCAL_EDGE_TOKEN` (docs/android.md § Custom server).
 */
data class CustomEdge(val url: String, val token: String) {
    companion object {
        /** Why [url] / [token] can't be used, or null when they can (the engine's own checks). */
        fun problem(url: String, token: String): String? {
            val u = url.trim()
            return when {
                !(u.startsWith("http://") || u.startsWith("https://")) -> "The server URL must start with http:// or https://"
                runCatching { java.net.URI(u).host }.getOrNull().isNullOrEmpty() -> "That URL has no host."
                token.trim().length < 16 -> "The token must be at least 16 characters."
                !token.trim().all { it.isLetterOrDigit() && it.code < 128 || it in "._~-" } ->
                    "The token may only use letters, digits and . _ ~ -"
                else -> null
            }
        }

        fun of(url: String, token: String) = CustomEdge(url.trim().trimEnd('/'), token.trim())
    }
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
