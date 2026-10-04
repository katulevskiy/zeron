package sh.zeron.android.voice.screen

import android.app.*
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.Bitmap
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.ImageReader
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.*
import android.view.WindowManager
import androidx.core.app.NotificationCompat
import sh.zeron.android.MainActivity
import sh.zeron.android.ZeronApplication

/** Captures locally at <=4 fps, retains one frame, sends changed frames at most
 * every two seconds with one transfer in flight. Audio uses its own service. */
class ScreenProjectionService : Service() {
    private var projection: MediaProjection? = null
    private var display: VirtualDisplay? = null
    private var reader: ImageReader? = null
    private val handler = Handler(Looper.getMainLooper())
    private var lastFrame = 0L
    private val screen get() = (application as ZeronApplication).existingModel?.jarvis?.screen
    private val receiver = object : android.content.BroadcastReceiver() {
        override fun onReceive(context: android.content.Context?, intent: Intent?) { screen?.stop(); stopSelf() }
    }
    override fun onCreate() {
        super.onCreate()
        androidx.core.content.ContextCompat.registerReceiver(this, receiver, android.content.IntentFilter(Intent.ACTION_SCREEN_OFF), androidx.core.content.ContextCompat.RECEIVER_NOT_EXPORTED)
    }
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == STOP) { screen?.stopSharing(); stopSelf(); return START_NOT_STICKY }
        if (projection != null) return START_NOT_STICKY
        val generation = intent?.getLongExtra("generation", 0) ?: 0
        val jarvis = (application as ZeronApplication).existingModel?.jarvis
        if (jarvis?.owns(generation) != true || !jarvis.screen.settings.contextEnabled.value) { stopSelf(); return START_NOT_STICKY }
        val data = if (Build.VERSION.SDK_INT >= 33) intent?.getParcelableExtra("consent", Intent::class.java) else @Suppress("DEPRECATION") intent?.getParcelableExtra("consent")
        if (data == null) { stopSelf(); return START_NOT_STICKY }
        try {
            getSystemService(NotificationManager::class.java).createNotificationChannel(NotificationChannel(CHANNEL, "Jarvis screen sharing", NotificationManager.IMPORTANCE_LOW))
            val stop = PendingIntent.getService(this, 0, Intent(this, ScreenProjectionService::class.java).setAction(STOP), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java).putExtra("route", "jarvis-settings"), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            val notification = NotificationCompat.Builder(this, CHANNEL).setSmallIcon(android.R.drawable.ic_menu_view)
                .setContentTitle("Jarvis is viewing your screen").setContentText("Changed screenshots are sent during this call")
                .setContentIntent(open).addAction(0, "Stop sharing", stop).setOngoing(true).setSilent(true).setVisibility(NotificationCompat.VISIBILITY_PRIVATE).build()
            startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
            val capture = getSystemService(MediaProjectionManager::class.java).getMediaProjection(intent?.getIntExtra("result", Activity.RESULT_OK) ?: Activity.RESULT_OK, data) ?: error("Screen capture unavailable")
            projection = capture
            capture.registerCallback(object : MediaProjection.Callback() {
                override fun onStop() { screen?.projectionEnded(); stopSelf() }
                override fun onCapturedContentResize(width: Int, height: Int) { resize(width, height) }
            }, handler)
            check(jarvis.screen.projectionStarted(generation))
            val metrics = android.util.DisplayMetrics()
            @Suppress("DEPRECATION") getSystemService(WindowManager::class.java).defaultDisplay.getRealMetrics(metrics)
            reader = newReader(metrics.widthPixels, metrics.heightPixels)
            display = capture.createVirtualDisplay("Jarvis screen", metrics.widthPixels, metrics.heightPixels, metrics.densityDpi,
                DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, reader!!.surface, null, handler)
        } catch (_: Exception) { screen?.projectionEnded(); screen?.report("Couldn't start screen sharing. Try again from the active call."); stopSelf() }
        return START_NOT_STICKY
    }
    private fun newReader(width: Int, height: Int): ImageReader = ImageReader.newInstance(width, height, android.graphics.PixelFormat.RGBA_8888, 2).apply {
        setOnImageAvailableListener({ source ->
            val image = try { source.acquireLatestImage() } catch (_: Exception) { null } ?: return@setOnImageAvailableListener
            image.use {
                if (SystemClock.elapsedRealtime() - lastFrame < 250) return@use
                lastFrame = SystemClock.elapsedRealtime()
                try {
                    val plane = it.planes[0]
                    val paddedWidth = it.width + (plane.rowStride - plane.pixelStride * it.width) / plane.pixelStride
                    val padded = Bitmap.createBitmap(paddedWidth, it.height, Bitmap.Config.ARGB_8888)
                    padded.copyPixelsFromBuffer(plane.buffer)
                    val cropped = Bitmap.createBitmap(padded, 0, 0, it.width, it.height)
                    if (cropped !== padded) padded.recycle()
                    screen?.frame(cropped) ?: cropped.recycle()
                } catch (_: Exception) { }
            }
        }, handler)
    }
    private fun resize(width: Int, height: Int) {
        if (width <= 0 || height <= 0 || display == null) return
        val old = reader
        val next = newReader(width, height)
        display?.resize(width, height, resources.displayMetrics.densityDpi)
        display?.surface = next.surface
        reader = next; old?.close()
    }
    override fun onDestroy() {
        unregisterReceiver(receiver)
        display?.release(); display = null
        reader?.close(); reader = null
        projection?.stop(); projection = null
        screen?.projectionEnded()
        super.onDestroy()
    }
    override fun onBind(intent: Intent?): IBinder? = null
    companion object { private const val CHANNEL = "jarvis-screen"; private const val ID = 7002; private const val STOP = "sh.zeron.android.jarvis.STOP_SCREEN" }
}
