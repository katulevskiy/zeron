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
 * Local session notifications (Android has no push; the device's client keeps
 * syncing while its engine runs): the desktop's rule — a run finished, needs
 * your input, or failed — posted only while Zeron is in the background.
 * Tapping one opens the session.
 */
class Notifier(private val context: Context) {
    enum class Kind { Done, Input, Failed }

    init {
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Sessions", NotificationManager.IMPORTANCE_HIGH).apply {
                description = "When a session finishes, needs you or fails"
            },
        )
        manager.createNotificationChannel(
            NotificationChannel(DOWNLOADS_CHANNEL, "Downloads", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Files saved to Downloads from your devices"
            },
        )
    }

    val permitted: Boolean
        get() = Build.VERSION.SDK_INT < 33 ||
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    fun onWorkspace(previous: WorkspaceSnapshot?, next: WorkspaceSnapshot, foreground: Boolean) {
        if (previous == null || foreground || !permitted) return
        val before = previous.allRows().associate { it.id to it.hostIndicator }
        for (row in next.allRows()) {
            val kind = transition(before[row.id] ?: continue, row.hostIndicator) ?: continue
            post(row, kind)
        }
    }

    private fun post(row: SessionRow, kind: Kind) {
        val open = Intent(context, MainActivity::class.java)
            .putExtra("route", "chat:${row.id}")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
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

    /** Save to Downloads from the developer tools: progress, then where it landed. */
    fun download(id: String, title: String, text: String, fraction: Float?, done: Boolean, open: Intent? = null) {
        val tap = open?.let {
            PendingIntent.getActivity(context, "download:$id".hashCode(), it.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        }
        val n = NotificationCompat.Builder(context, DOWNLOADS_CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_zeron)
            .setContentTitle(title)
            .setContentText(text)
            .setCategory(if (done) NotificationCompat.CATEGORY_STATUS else NotificationCompat.CATEGORY_PROGRESS)
            .apply {
                if (!done) setProgress(100, ((fraction ?: 0f) * 100).toInt(), fraction == null).setOngoing(true).setOnlyAlertOnce(true).setSilent(true)
                else setAutoCancel(true)
                if (tap != null) setContentIntent(tap)
            }
            .build()
        if (!permitted) return
        try {
            NotificationManagerCompat.from(context).notify("download:$id".hashCode(), n)
        } catch (_: SecurityException) {
        }
    }

    companion object {
        const val CHANNEL = "sessions"
        const val DOWNLOADS_CHANNEL = "downloads"

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
    (front.pinned + front.sections.flatMap { it.sessions } + front.recent + projects.flatMap { it.sessions } + projectless).distinctBy { it.id }
