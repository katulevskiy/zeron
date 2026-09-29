package sh.zeron.android.core

import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.ComposerState
import uniffi.zeron_core.HostCapabilities
import uniffi.zeron_core.HostInfo
import uniffi.zeron_core.LiveStatus
import uniffi.zeron_core.ProjectRef
import uniffi.zeron_core.RoomState
import uniffi.zeron_core.SendState
import uniffi.zeron_core.SessionRow

/** Plain-data fixtures for the generated UniFFI records (no native library needed). */
object Fixtures {
    fun row(
        id: String,
        indicator: ChatIndicator = ChatIndicator.IDLE,
        unseen: Boolean = false,
        sendState: SendState? = null,
        pinned: Boolean = false,
        project: String? = "zeron",
    ) = SessionRow(
        id = id,
        revision = 1u,
        title = "Session $id",
        hasTitle = true,
        preview = null,
        project = project?.let { ProjectRef("p-$it", it, 2u) },
        deviceId = "mac",
        deviceName = "Mac",
        deviceOnline = true,
        harness = "claude-code",
        harnessLabel = "Claude Code",
        model = null,
        modelLabel = null,
        reasoning = null,
        branch = "main",
        cwd = null,
        indicator = indicator,
        hostIndicator = indicator,
        workingSinceMs = null,
        lastActivityMs = 0,
        timeLabel = "4m",
        createdAtMs = 0,
        unseen = unseen,
        archived = false,
        pinned = pinned,
        sectionId = null,
        pullRequest = null,
        sendState = sendState,
        parentChatId = null,
        roomGen = 2u,
    )

    fun composer(
        running: Boolean = false,
        sendState: SendState? = null,
        roomConnected: Boolean = true,
        retryAtMs: Long? = null,
        queueError: String? = null,
    ) = ComposerState(
        chatId = "c1",
        revision = 1u,
        title = "Fix the build",
        host = HostInfo("mac", "Mac", true, HostCapabilities(true, true, true, true, true, true, true)),
        live = LiveStatus(if (running) ChatIndicator.WORKING else ChatIndicator.IDLE, running, null, false, running),
        queue = emptyList(),
        pendingSends = emptyList(),
        sendState = sendState,
        deliveryDegraded = false,
        room = RoomState(roomConnected, retryAtMs, false),
        transferProgress = null,
        openInput = null,
        queueError = queueError,
        lastSubmittedMessageId = null,
        contextUsage = null,
    )
}
