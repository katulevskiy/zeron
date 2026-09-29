package sh.zeron.android.core

/**
 * Pure formatting and parsing helpers shared by several screens (JVM-unit
 * tested in `FormattersTest`).
 */
object Formatters {
    /** `42s` / `3m 7s` / `1h 4m` — the working pill and trailer. */
    fun elapsed(seconds: Long): String = when {
        seconds < 60 -> "${seconds}s"
        seconds < 3600 -> "${seconds / 60}m ${seconds % 60}s"
        else -> "${seconds / 3600}h ${seconds / 60 % 60}m"
    }

    /** "2 working · 1 needs you" (empty when nothing is live). */
    fun liveSummary(working: Int, awaiting: Int): String = buildList {
        if (working > 0) add("$working working")
        if (awaiting > 0) add("$awaiting need${if (awaiting == 1) "s" else ""} you")
    }.joinToString(" · ")

    /** Parent of a POSIX folder path (`null` at the root or for a bare name). */
    fun parentPath(path: String): String? {
        val trimmed = if (path.length > 1 && path.endsWith("/")) path.dropLast(1) else path
        if (trimmed == "/" || !trimmed.contains('/')) return null
        val up = trimmed.substringBeforeLast('/')
        return up.ifEmpty { "/" }
    }

    fun joinPath(dir: String, name: String): String = if (dir.endsWith("/")) dir + name else "$dir/$name"

    fun lastComponent(path: String): String = path.trimEnd('/').substringAfterLast('/').ifEmpty { path }

    /**
     * Folder name a `git clone <url>` creates: the last path segment without
     * `.git` (`git@host:org/repo.git` → `repo`). `null` for nothing usable.
     */
    fun repoName(url: String): String? {
        val trimmed = url.trim().trimEnd('/').removeSuffix(".git")
        // A bare word is not a repository URL.
        if (!trimmed.contains('/') && !trimmed.contains(':')) return null
        val name = trimmed.substringAfterLast('/').substringAfterLast(':')
        return name.takeIf { it.isNotEmpty() && it.all { c -> c.isLetterOrDigit() || c in "._-" } }
    }

    /** Single-quote a string for `sh -c` (runtime exec). */
    fun shellQuote(s: String): String = "'" + s.replace("'", "'\\''") + "'"

    /** A context-usage chip title, or null below 50 %. */
    fun contextChip(tokens: Long?, window: Long?): String? {
        if (tokens == null || window == null || window <= 0) return null
        val fraction = tokens.toDouble() / window
        return if (fraction >= 0.5) "${Math.round(fraction * 100)}% context" else null
    }

    fun capitalized(s: String): String = s.replaceFirstChar { it.uppercase() }
}
