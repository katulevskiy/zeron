package sh.zeron.android.voice

import android.app.*
import android.content.*
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import sh.zeron.android.MainActivity
import sh.zeron.android.ZeronApplication

/** Android's microphone foreground service keeps an explicitly started call alive. */
class JarvisService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val controller get() = (application as ZeronApplication).model.jarvis
    private var generation = 0L

    override fun onCreate() {
        super.onCreate()
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(CHANNEL, "Jarvis calls", NotificationManager.IMPORTANCE_LOW))
        if (android.os.Build.VERSION.SDK_INT >= 30) startForeground(ID, notification(false), ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
        else startForeground(ID, notification(false))
        ready.value = -1
        scope.launch { controller.state.collect { state ->
            if (state.live) manager.notify(ID, notification(state.muted))
        } }
    }

    private fun notification(muted: Boolean): Notification {
        val open = Intent(this, MainActivity::class.java).putExtra("route", "jarvis")
        fun action(name: String, code: Int) = PendingIntent.getService(this, code,
            Intent(this, JarvisService::class.java).setAction(name).putExtra("generation", generation), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        return NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setContentTitle(if (muted) "Jarvis · microphone muted" else "Jarvis call")
            .setContentText("Tap to return to the call")
            .setContentIntent(PendingIntent.getActivity(this, 0, open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT))
            .setOngoing(true).setSilent(true).setCategory(NotificationCompat.CATEGORY_CALL)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .addAction(0, if (muted) "Unmute" else "Mute", action(MUTE, 1))
            .addAction(0, "Hang up", action(STOP, 2)).build()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val id = intent?.getLongExtra("generation", 0) ?: 0
        if (intent?.action == null && controller.owns(id)) { generation = id; ready.value = id }
        if (controller.state.value.live) getSystemService(NotificationManager::class.java).notify(ID, notification(controller.state.value.muted))
        if (controller.owns(id)) when (intent?.action) { MUTE -> controller.toggleMute(); STOP -> controller.stop() }
        if (!controller.state.value.live) stopSelf()
        return START_NOT_STICKY
    }
    override fun onTaskRemoved(rootIntent: Intent?) { controller.stopIfOwned(generation); stopSelf() }
    override fun onDestroy() {
        if (ready.value == generation || ready.value == -1L) ready.value = 0
        controller.stopIfOwned(generation)
        scope.cancel()
        super.onDestroy()
    }
    override fun onBind(intent: Intent?): IBinder? = null

    companion object {
        private const val CHANNEL = "jarvis-calls"
        private const val ID = 7001
        private const val MUTE = "sh.zeron.android.jarvis.MUTE"
        private const val STOP = "sh.zeron.android.jarvis.STOP"
        private val ready = MutableStateFlow(0L)
        suspend fun ensureStarted(context: Context, generation: Long) {
            // An old service must finish destruction before a new call can
            // acquire it; stopService is asynchronous.
            if (ready.value != 0L && ready.value != generation) withTimeout(5_000) { ready.first { it == 0L } }
            ContextCompat.startForegroundService(context, Intent(context, JarvisService::class.java).putExtra("generation", generation))
            withTimeout(5_000) { ready.first { it == generation } }
        }
        fun end(context: Context) { context.stopService(Intent(context, JarvisService::class.java)) }
    }
}
