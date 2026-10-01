package sh.zeron.android.ui

import org.junit.Assert.*
import org.junit.Test
import uniffi.zeron_core.*

class SessionWorkingFilterTest {
    private fun row(id: String, status: ChatIndicator = ChatIndicator.IDLE, children: UInt = 0u, callbacks: UInt = 0u) = SessionRow(
        id = id, revision = 0uL, title = id, hasTitle = true, preview = null, project = null,
        deviceId = "host", deviceName = "Host", deviceOnline = true, harness = "claude-code", harnessLabel = "Claude Code",
        model = null, modelLabel = null, reasoning = null, branch = null, cwd = null,
        indicator = status, hostIndicator = status, workingSinceMs = null, lastActivityMs = 0, timeLabel = "now",
        createdAtMs = 0, unseen = false, archived = false, pinned = false, sectionId = null, pullRequest = null,
        sendState = null, parentChatId = null, roomGen = 2u, runningSubagents = children, pendingCallbacks = callbacks,
    )

    @Test fun filterAndSummaryIncludeIdleParentsAndCallbacksAndCountDuplicateRowsOnce() {
        val main = row("main", ChatIndicator.WORKING, children = 2u)
        val children = row("children", children = 4u)
        val callback = row("callback", callbacks = 1u)
        val idle = row("idle")
        val ws = WorkspaceSnapshot(
            revision = 0uL, computedAtMs = 0, pinsReady = true, synced = true,
            front = FrontPage(listOf(main), listOf(SectionView("s", "Work", false, listOf(children))), listOf(main, children, callback, idle)),
            projects = emptyList(), projectless = emptyList(), pullRequests = PullRequestGroups(emptyList(), emptyList(), emptyList()),
            archived = emptyList(), devices = emptyList(),
        )
        assertEquals(listOf("main", "children", "callback"), workingRows(ws).map { it.id })
        assertEquals(3 to 0, liveCounts(ws))
        assertEquals("3 working", liveSummary(ws))
        val needsInput = row("input", ChatIndicator.AWAITING_INPUT, children = 1u)
        val changed = ws.copy(front = FrontPage(emptyList(), emptyList(), listOf(needsInput, idle)))
        assertEquals(1 to 1, liveCounts(changed))
        assertEquals("1 needs you", liveSummary(changed))
    }
}
