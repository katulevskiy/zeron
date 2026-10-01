package sh.zeron.android.core

import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.SessionRow

object SessionActivity {
    enum class Shape(val description: String) {
        MainRunning("Main agent running"),
        SubagentsRunning("Subagents running; main agent idle"),
        CallbackWaiting("Waiting for a background callback"),
    }

    fun shape(indicator: ChatIndicator, subagents: UInt, callbacks: UInt): Shape? = when (indicator) {
        ChatIndicator.WORKING -> Shape.MainRunning
        ChatIndicator.AWAITING_INPUT, ChatIndicator.ERRORED -> null
        else -> when {
            subagents > 0u -> Shape.SubagentsRunning
            callbacks > 0u -> Shape.CallbackWaiting
            else -> null
        }
    }

    fun isWorking(indicator: ChatIndicator, subagents: UInt, callbacks: UInt): Boolean =
        indicator == ChatIndicator.WORKING || subagents > 0u || callbacks > 0u

    fun isWorking(row: SessionRow): Boolean = isWorking(row.indicator, row.runningSubagents, row.pendingCallbacks)

    /** Overflow uses an icon; neither it nor the hidden zero badge has text. */
    fun badgeLabel(count: UInt): String? = when {
        count == 0u || count > 99u -> null
        else -> count.toString()
    }
}
