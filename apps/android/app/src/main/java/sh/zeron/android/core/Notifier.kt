package sh.zeron.android.core

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import sh.zeron.android.MainActivity
import sh.zeron.android.R
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.SessionRow
import uniffi.zeron_core.WorkspaceSnapshot

/**
 * Local session notifications for the on-device engine (there is no push in
 * Phone mode — the engine is right here): the desktop's rule — a run
 * finished, needs your input, or failed — posted only while Zeron is in the
 * background. Tapping one opens the session.
 */
class Notifier(private val context: Context) {
    enum class Kind { Done, Input, Failed }

    init {
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL, "Sessions", NotificationManager.IMPORTANCE_HIGH).apply {
                description = "When a session finishes, needs you or fails"
            },
        )
    }

    val permitted: Boolean
        get() = Build.VERSION.SDK_INT < 33 ||
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    fun onWorkspace(previous: WorkspaceSnapshot?, next: WorkspaceSnapshot, foreground: Boolean) {
        if (previous == null || foreground || !permitted) return
        if (!Prefs(context).notifications) return
        val before = previous.allRows().associate { it.id to it.hostIndicator }
        for (row in next.allRows()) {
            val kind = transition(before[row.id] ?: continue, row.hostIndicator) ?: continue
            post(row, kind)
        }
    }

    private fun post(row: SessionRow, kind: Kind) {
        val open = Intent(context, MainActivity::class.java).apply {
            action = Intent.ACTION_VIEW
            putExtra(MainActivity.EXTRA_CHAT, row.id)
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP
        }
        val pending = PendingIntent.getActivity(context, row.id.hashCode(), open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val body = when (kind) {
            Kind.Done -> "Finished"
            Kind.Input -> "Needs your input"
            Kind.Failed -> "Run failed"
        }
        val n = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_zeron)
            .setContentTitle(row.title)
            .setContentText(listOfNotNull(body, row.project?.name).joinToString(" · "))
            .setAutoCancel(true)
            .setContentIntent(pending)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .build()
        try {
            NotificationManagerCompat.from(context).notify(row.id.hashCode(), n)
        } catch (_: SecurityException) {
            // Permission revoked between the check and the post.
        }
    }

    companion object {
        const val CHANNEL = "sessions"

        /** Which notification (if any) a host status change deserves. */
        fun transition(before: ChatIndicator, after: ChatIndicator): Kind? = when {
            before == after -> null
            after == ChatIndicator.AWAITING_INPUT -> Kind.Input
            after == ChatIndicator.ERRORED -> Kind.Failed
            before == ChatIndicator.WORKING && (after == ChatIndicator.IDLE || after == ChatIndicator.COMPLETED) -> Kind.Done
            else -> null
        }
    }
}

fun WorkspaceSnapshot.allRows(): List<SessionRow> =
    (front.pinned + front.sections.flatMap { it.sessions } + front.recent + projectless).distinctBy { it.id }
