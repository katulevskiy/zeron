package sh.zeron.android.core

/**
 * `@file` mentions: the composer shows `@name` tokens; on send they become
 * the canonical `[name](zeron-file:path)` links the host resolves (the link
 * format comes from the core — `file_mention_link` — injected as [link]).
 */
class MentionIndex(private val link: (path: String, isDir: Boolean) -> String) {
    data class File(val path: String, val isDir: Boolean)

    private val tokens = LinkedHashMap<String, File>()

    val liveTokens: Set<String> get() = tokens.keys

    /** Token for a file, disambiguated by parent folder on basename clashes. */
    fun token(file: File): String {
        val parts = file.path.trim('/').split('/').filter { it.isNotEmpty() }
        var token = "@" + (parts.lastOrNull() ?: file.path)
        val existing = tokens[token]
        if (existing != null && existing.path != file.path && parts.size > 1) {
            token = "@" + parts.takeLast(2).joinToString("/")
        }
        tokens[token] = file
        return token
    }

    /** Replace live tokens with canonical links (longest first, so `@a/b` beats `@b`). */
    fun encode(text: String): String {
        var out = text
        for ((token, file) in tokens.entries.sortedByDescending { it.key.length }) {
            if (out.contains(token)) out = out.replace(token, link(file.path, file.isDir))
        }
        return out
    }

    fun reset() = tokens.clear()

    companion object {
        /**
         * The `@query` being typed at `cursor` (UTF-16 index), if any: an `@`
         * at a word start with no whitespace between it and the cursor.
         * Returns the token's range start and the query text.
         */
        fun activeQuery(text: String, cursor: Int): Pair<Int, String>? {
            if (cursor > text.length) return null
            var i = cursor - 1
            while (i >= 0) {
                val c = text[i]
                if (c == '@') {
                    val before = if (i > 0) text[i - 1] else ' '
                    if (before != ' ' && before != '\n') return null
                    return i to text.substring(i + 1, cursor)
                }
                if (c == ' ' || c == '\n') return null
                i--
            }
            return null
        }
    }
}

/** Queue-edit text surgery (see iOS `CoreSessionSource.replacingVisible`). */
object QueueText {
    /**
     * The row's raw text with its visible part replaced, keeping the hidden
     * context (Appshot context, attachment trailer) after it. Blank edits
     * stay blank (the host removes the row).
     */
    fun replacingVisible(raw: String, visible: String, edited: String): String {
        if (edited.isBlank() || raw == visible) return edited
        val body = raw.trimStart()
        if (body.startsWith(visible)) return edited + body.substring(visible.length)
        // Attachment-only rows show a placeholder: all of the raw text is context.
        return if (body.isEmpty()) edited else edited + "\n\n" + body
    }
}
