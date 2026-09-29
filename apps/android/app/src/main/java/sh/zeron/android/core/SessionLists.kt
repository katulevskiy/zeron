package sh.zeron.android.core

import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.FrontPage
import uniffi.zeron_core.SendState
import uniffi.zeron_core.SessionRow

/** A live marker on a folded section header (something inside is running / asking). */
enum class LiveMark { Working, Input }

/** One row of a session list: a foldable section header or a session. */
sealed interface ListItem {
    /** Stable, unique key (lazy lists and animations key on it). */
    val key: String

    data class Header(
        val id: String,
        val title: String,
        val count: Int,
        val collapsed: Boolean,
        val live: LiveMark?,
    ) : ListItem {
        override val key get() = "h:$id"
    }

    data class Session(val row: SessionRow, val section: String) : ListItem {
        override val key get() = "s:${row.id}"
    }
}

/**
 * What a row's trailing corner says, desktop precedence: a failed send beats
 * everything; then the live state; an unseen finished run reads "Done";
 * otherwise nothing (the row shows its time label).
 */
enum class Corner(val word: String) {
    SendFailed("Failed"),
    Working("Working"),
    Input("Input"),
    Failed("Failed"),
    Done("Done"),
}

/**
 * Pure list building for the sessions screens (JVM-unit tested): the front
 * page laid out like the desktop sidebar — Pinned and the user's sections as
 * foldable groups, then Recent.
 */
object SessionLists {
    const val PINNED = "pinned"
    const val RECENT = "recent"
    const val ARCHIVED = "archived"

    fun corner(row: SessionRow): Corner? {
        if (row.sendState == SendState.FAILED) return Corner.SendFailed
        return when (row.indicator) {
            ChatIndicator.WORKING -> Corner.Working
            ChatIndicator.AWAITING_INPUT -> Corner.Input
            ChatIndicator.ERRORED -> Corner.Failed
            ChatIndicator.COMPLETED -> if (row.unseen) Corner.Done else null
            ChatIndicator.IDLE -> null
        }
    }

    fun live(rows: List<SessionRow>): LiveMark? = when {
        rows.any { it.indicator == ChatIndicator.WORKING } -> LiveMark.Working
        rows.any { it.indicator == ChatIndicator.AWAITING_INPUT } -> LiveMark.Input
        else -> null
    }

    /**
     * The front page. Section fold state comes from the synced section rows;
     * Pinned and Recent fold locally (`localCollapsed`). A session appears
     * once (first group wins) so keys stay unique. Recent gets a header only
     * when something is above it.
     */
    fun frontPage(front: FrontPage, localCollapsed: Set<String>): List<ListItem> {
        val out = ArrayList<ListItem>()
        val seen = HashSet<String>()
        fun group(id: String, title: String?, rows: List<SessionRow>, collapsed: Boolean) {
            val unique = rows.filter { it.id !in seen }
            if (title != null) out.add(ListItem.Header(id, title, unique.size, collapsed, live(unique)))
            unique.forEach { seen.add(it.id) }
            if (title == null || !collapsed) unique.forEach { out.add(ListItem.Session(it, id)) }
        }
        if (front.pinned.isNotEmpty()) group(PINNED, "Pinned", front.pinned, PINNED in localCollapsed)
        for (s in front.sections) group(s.id, s.name, s.sessions, s.collapsed)
        val recentTitle = if (out.isEmpty()) null else "Recent"
        group(RECENT, recentTitle, front.recent, RECENT in localCollapsed)
        return out
    }

    /** Live counts for the "New session" capsule ("2 working · 1 needs you"). */
    fun liveCounts(front: FrontPage): Pair<Int, Int> {
        val rows = (front.pinned + front.sections.flatMap { it.sessions } + front.recent).distinctBy { it.id }
        return rows.count { it.indicator == ChatIndicator.WORKING } to rows.count { it.indicator == ChatIndicator.AWAITING_INPUT }
    }

    /** Neighbours for a pin move (`move_pin(id, after, before)`), or null at the ends. */
    fun pinMove(order: List<String>, id: String, delta: Int): Pair<String?, String?>? {
        val from = order.indexOf(id)
        if (from < 0) return null
        val to = from + delta
        if (to < 0 || to >= order.size) return null
        val rest = order.toMutableList().apply { removeAt(from) }
        rest.add(to, id)
        return rest.getOrNull(to - 1) to rest.getOrNull(to + 1)
    }
}
