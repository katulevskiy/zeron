package sh.zeron.android.voice

import android.content.Context
import android.media.*
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.hardware.*

/** Focus and routing belong to this call, and are restored on every exit path. */
class VoiceAudioRoute(context: Context, private val lost: () -> Unit) : SensorEventListener {
    private val audio = context.getSystemService(AudioManager::class.java)
    private val sensors = context.getSystemService(SensorManager::class.java)
    private val main = Handler(Looper.getMainLooper())
    private val power = context.getSystemService(PowerManager::class.java)
    private val proximity = if (power.isWakeLockLevelSupported(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK))
        power.newWakeLock(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK, "zeron:jarvis-proximity") else null
    private var prepared = false
    private var active = false
    private var speaker = true
    private var near = false
    private var oldMode = AudioManager.MODE_NORMAL
    private var oldSpeaker = false
    private var oldDevice: AudioDeviceInfo? = null
    private val focus = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_EXCLUSIVE)
        .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
        .setOnAudioFocusChangeListener({ change ->
            if (prepared && change < 0) lost()
        }, main).build()
    private val devices = object : AudioDeviceCallback() {
        override fun onAudioDevicesAdded(addedDevices: Array<out AudioDeviceInfo>) { route() }
        override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>) { route() }
    }

    fun prepare() {
        check(!prepared)
        check(audio.mode != AudioManager.MODE_IN_CALL) { "A phone call is active" }
        oldMode = audio.mode
        oldSpeaker = audio.isSpeakerphoneOn
        if (Build.VERSION.SDK_INT >= 31) oldDevice = audio.communicationDevice
        check(audio.requestAudioFocus(focus) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED)
        prepared = true
        audio.registerAudioDeviceCallback(devices, main)
    }

    fun activate() {
        check(prepared)
        if (!active) {
            active = true
            audio.mode = AudioManager.MODE_IN_COMMUNICATION
            sensors.getDefaultSensor(Sensor.TYPE_PROXIMITY)?.let {
                sensors.registerListener(this, it, SensorManager.SENSOR_DELAY_NORMAL, main)
            }
        }
        route()
    }

    fun toggleSpeaker() { speaker = !speaker; route() }

    private fun route() {
        if (!active) return
        if (Build.VERSION.SDK_INT >= 31) {
            val available = audio.availableCommunicationDevices
            val external = available.firstOrNull { it.type in setOf(
                AudioDeviceInfo.TYPE_BLUETOOTH_SCO, AudioDeviceInfo.TYPE_BLE_HEADSET,
                AudioDeviceInfo.TYPE_WIRED_HEADSET, AudioDeviceInfo.TYPE_WIRED_HEADPHONES,
                AudioDeviceInfo.TYPE_USB_HEADSET, AudioDeviceInfo.TYPE_USB_DEVICE,
                AudioDeviceInfo.TYPE_BLE_SPEAKER) }
            val wanted = if (speaker && !near) AudioDeviceInfo.TYPE_BUILTIN_SPEAKER else AudioDeviceInfo.TYPE_BUILTIN_EARPIECE
            (external ?: available.firstOrNull { it.type == wanted }
                ?: available.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER })?.let {
                if (!audio.setCommunicationDevice(it)) { lost(); return }
            }
            proximity(near && external == null && wanted == AudioDeviceInfo.TYPE_BUILTIN_EARPIECE)
        } else {
            @Suppress("DEPRECATION")
            audio.isSpeakerphoneOn = speaker && !near && !audio.isWiredHeadsetOn && !audio.isBluetoothScoOn
            proximity(near && !audio.isWiredHeadsetOn && !audio.isBluetoothScoOn)
        }
    }

    @android.annotation.SuppressLint("WakelockTimeout")
    private fun proximity(off: Boolean) {
        // Screen-off only (not a CPU lock), scoped to the active call.
        if (off && proximity?.isHeld == false) proximity.acquire()
        else if (!off && proximity?.isHeld == true) proximity.release()
    }

    override fun onSensorChanged(event: SensorEvent) {
        near = event.values.firstOrNull()?.let { it < event.sensor.maximumRange } ?: false
        route()
    }
    override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) = Unit

    fun close() {
        if (!prepared) return
        prepared = false
        val wasActive = active
        active = false
        proximity(false)
        sensors.unregisterListener(this)
        audio.unregisterAudioDeviceCallback(devices)
        if (wasActive && audio.mode == AudioManager.MODE_IN_COMMUNICATION) {
            if (Build.VERSION.SDK_INT >= 31) {
                audio.clearCommunicationDevice()
                oldDevice?.takeIf { device -> audio.availableCommunicationDevices.any { it.id == device.id } }
                    ?.let { audio.setCommunicationDevice(it) }
            }
            else audio.isSpeakerphoneOn = oldSpeaker
            audio.mode = oldMode
        }
        audio.abandonAudioFocusRequest(focus)
    }
}
