package sh.zeron.android.core

import android.content.Context
import kotlinx.coroutines.flow.StateFlow
import sh.zeron.runtime.ExecResult
import sh.zeron.runtime.RuntimeController
import sh.zeron.runtime.RuntimeState
import sh.zeron.runtime.ZeronRuntime

/**
 * The on-device engine (`:runtime`) as the UI sees it: state, lifecycle,
 * logs and one-off guest commands (docs/android.md § Runtime API).
 */
class PhoneEngine(context: Context) {
    private val runtime: RuntimeController = ZeronRuntime.get(context)

    val state: StateFlow<RuntimeState> get() = runtime.state
    val isSupportedAbi: Boolean get() = runtime.isSupportedAbi

    fun start() = runtime.start()
    fun stop() = runtime.stop()
    suspend fun reset() = runtime.reset()
    /** The runtime's log with the engine's terminal colours stripped. */
    fun logTail(lines: Int = 200): String = stripAnsi(runtime.logTail(lines))
    suspend fun exec(command: String, timeoutMs: Long = 600_000): ExecResult = runtime.exec(command, timeoutMs = timeoutMs)

    /** `git clone` into the projects root; returns the checkout path. */
    suspend fun clone(url: String): Result<String> {
        val name = repoName(url) ?: return Result.failure(IllegalArgumentException("That doesn't look like a git URL."))
        val dest = "$PROJECTS_ROOT/$name"
        val cmd = "mkdir -p ${shellQuote(PROJECTS_ROOT)} && git clone ${shellQuote(url.trim())} ${shellQuote(dest)} 2>&1"
        return run(cmd, dest, "git clone failed")
    }

    /** An empty git repository under the projects root; returns its path. */
    suspend fun newProject(name: String): Result<String> {
        val folder = folderName(name) ?: return Result.failure(IllegalArgumentException("Use letters, digits, dots, dashes or underscores."))
        val dest = "$PROJECTS_ROOT/$folder"
        return run("mkdir -p ${shellQuote(dest)} && cd ${shellQuote(dest)} && git init -q 2>&1", dest, "Couldn't create the folder")
    }

    private suspend fun run(cmd: String, dest: String, fallback: String): Result<String> {
        val result = runCatching { runtime.exec(cmd, timeoutMs = 15 * 60_000) }.getOrElse { return Result.failure(it) }
        return if (result.exitCode == 0) {
            Result.success(dest)
        } else {
            Result.failure(RuntimeException(result.output.lines().lastOrNull { it.isNotBlank() } ?: fallback))
        }
    }

    companion object {
        /** Projects on the phone's engine live here (docs/android.md § Guest layout). */
        const val PROJECTS_ROOT = "/home/zeron/projects"

        /**
         * Folder name a `git clone <url>` creates: the last path segment without
         * `.git` (`git@host:org/repo.git` → `repo`). `null` for nothing usable.
         */
        fun repoName(url: String): String? {
            val trimmed = url.trim().trimEnd('/').removeSuffix(".git")
            // A bare word is not a repository URL.
            if (!trimmed.contains('/') && !trimmed.contains(':')) return null
            return folderName(trimmed.substringAfterLast('/').substringAfterLast(':'))
        }

        /** A safe single folder name, or null. */
        fun folderName(name: String): String? =
            name.trim().takeIf { it.isNotEmpty() && it != "." && it != ".." && it.all { c -> c.isLetterOrDigit() || c in "._-" } }

        private val ansi = Regex("\u001B\\[[0-9;?]*[A-Za-z]")

        fun stripAnsi(s: String): String = s.replace(ansi, "")

        /** Single-quote a string for `sh -c`. */
        fun shellQuote(s: String): String = "'" + s.replace("'", "'\\''") + "'"

        fun stateLabel(s: RuntimeState): String = when (s) {
            RuntimeState.NotInstalled -> "Not set up"
            is RuntimeState.Bootstrapping -> "Setting up"
            RuntimeState.Starting -> "Starting"
            is RuntimeState.Running -> "Running"
            RuntimeState.Stopped -> "Stopped"
            is RuntimeState.Failed -> "Stopped with an error"
        }
    }
}
