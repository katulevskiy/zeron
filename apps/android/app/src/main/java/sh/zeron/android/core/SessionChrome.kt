package sh.zeron.android.core

import uniffi.zeron_core.ComposerState
import uniffi.zeron_core.Connectivity
import uniffi.zeron_core.ConnectivityState
import uniffi.zeron_core.PullRequestState
import uniffi.zeron_core.QueueGate
import uniffi.zeron_core.SendState
import uniffi.zeron_core.SessionRow

/** A context chip in the composer toolbar (model, effort, branch, PR, context). */
data class ComposerChip(
    val id: String,
    val title: String,
    /** Harness for a brand mark (model chip). */
    val harness: String? = null,
    val kind: Kind = Kind.Plain,
    val warn: Boolean = false,
    /** Project tone for a project chip's tile. */
    val colorIndex: Int? = null,
) {
    enum class Kind { Plain, Model, Effort, Branch, PrOpen, PrMerged, PrClosed, Context, Project, Host }
}

/** The small status capsule above the composer. */
sealed interface Banner {
    data object Offline : Banner
    data class Reconnecting(val seconds: Long) : Banner
    data object NotDelivered : Banner
    data class Failed(val message: String) : Banner
    data object Editing : Banner
}

data class QuestionModel(val id: String, val header: String, val text: String, val options: List<String>, val multiSelect: Boolean)

data class QueuedModel(val id: String, val text: String, val gate: String?)

/**
 * Composer-facing state of a session — everything the session screen
 * renders except the transcript (which flows Rust→Rust into layout).
 */
data class SessionChrome(
    val title: String = "",
    val subtitle: String = "",
    val running: Boolean = false,
    val canSteer: Boolean = false,
    val placeholder: String = "Message",
    val chips: List<ComposerChip> = emptyList(),
    val banner: Banner? = null,
    val questions: Pair<String, List<QuestionModel>>? = null,
    val queue: List<QueuedModel> = emptyList(),
    val error: String? = null,
    val uploadProgress: Double? = null,
    val hostDevice: String = "",
) {
    /** Labels come from the core catalog (FFI); injected so the reducer stays pure. */
    interface Labels {
        fun reasoning(level: String): String
    }

    companion object {
        /** The reducer: composer state + session row + connectivity → chrome. */
        fun from(
            c: ComposerState,
            row: SessionRow?,
            connectivity: Connectivity?,
            sendFailure: String?,
            nowMs: Long,
            labels: Labels,
        ): SessionChrome {
            val project = row?.project?.name ?: "No project"
            val chips = buildList {
                val model = row?.modelLabel ?: row?.harnessLabel
                if (model != null) add(ComposerChip("model", model, harness = row?.harness ?: "claude-code", kind = ComposerChip.Kind.Model))
                val effort = row?.reasoning
                if (!effort.isNullOrEmpty()) add(ComposerChip("effort", labels.reasoning(effort), kind = ComposerChip.Kind.Effort))
                val pr = row?.pullRequest
                if (pr != null) {
                    val kind = when (pr.state) {
                        PullRequestState.OPEN -> ComposerChip.Kind.PrOpen
                        PullRequestState.MERGED -> ComposerChip.Kind.PrMerged
                        PullRequestState.CLOSED -> ComposerChip.Kind.PrClosed
                    }
                    add(ComposerChip("pr", "${pr.number}", kind = kind))
                } else if (!row?.branch.isNullOrEmpty()) {
                    add(ComposerChip("branch", row!!.branch!!, kind = ComposerChip.Kind.Branch))
                }
                val usage = c.contextUsage
                val ctx = Formatters.contextChip(usage?.tokens?.toLong(), usage?.window?.toLong())
                if (ctx != null) {
                    val fraction = usage!!.tokens!!.toDouble() / usage.window!!.toDouble()
                    add(ComposerChip("context", ctx, kind = ComposerChip.Kind.Context, warn = fraction >= 0.85))
                }
            }
            val banner: Banner? = when {
                sendFailure != null -> Banner.Failed(sendFailure)
                c.sendState == SendState.FAILED -> Banner.NotDelivered
                c.sendState == SendState.QUEUED -> Banner.Failed("${c.host.name ?: "Host"} is offline — will send when it's back")
                connectivity?.state == ConnectivityState.OFFLINE -> Banner.Offline
                !c.room.connected && c.room.retryAtMs != null ->
                    Banner.Reconnecting(maxOf(1L, (c.room.retryAtMs!! - nowMs) / 1000))
                else -> null
            }
            return SessionChrome(
                title = c.title,
                subtitle = c.host.name?.let { "$project @ $it" } ?: project,
                running = c.live.turnRunning,
                canSteer = c.host.capabilities.midTurnSteering ?: false,
                placeholder = "Message ${row?.harnessLabel ?: "the agent"}",
                chips = chips,
                banner = banner,
                questions = c.openInput?.let { input ->
                    input.requestId to input.questions.map { QuestionModel(it.id, it.header, it.question, it.options, it.multiSelect) }
                },
                queue = c.queue.map { q ->
                    val gate = when (val g = q.gate) {
                        is QueueGate.Editing -> if (g.mine) "Editing" else "Being edited"
                        is QueueGate.ReviewRequired -> "Needs review"
                        null -> if (q.actionPending) "Updating" else null
                    }
                    QueuedModel(q.id, q.visibleText.ifBlank { "Attachment" }, gate)
                },
                error = c.queueError,
                uploadProgress = c.transferProgress,
                hostDevice = c.host.deviceId,
            )
        }
    }
}
