package sh.zeron.android.feedback

import android.app.NotificationManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ApplicationInfo
import android.database.ContentObserver
import android.media.AudioManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import android.os.VibrationAttributes
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.provider.Settings
import android.util.Log
import android.view.View
import androidx.core.content.ContextCompat
import java.lang.ref.WeakReference

/** The one place the rest of the app reaches the engine without Compose (view models, `AppModel`, tools). */
object AppFeedback {
    @Volatile
    var current: Feedback = NoFeedback
}

/** One read of everything the system says about feedback; cached by [SystemEnvironment]. */
private class SystemSnapshot(
    val interactive: Boolean,
    val touchHaptics: Boolean,
    val touchSounds: Boolean,
    val ringerNormal: Boolean,
    val dnd: Boolean,
    val streamAudible: Boolean,
)

/**
 * The real [FeedbackEnvironment]: the activity lifecycle plus the system's sound, vibration and DND state.
 *
 * The system state costs several binder calls and settings reads, which would sit on the tap-to-sound path
 * if made per event. It is read into one snapshot that lives [TTL_MS] and is dropped at once when the system
 * says it changed (ringer, volume, interruption filter, screen, the two touch settings; see [watch]).
 */
class SystemEnvironment(private val context: Context, val vibrator: Vibrator?, clock: () -> Long = SystemClock::uptimeMillis) : FeedbackEnvironment {
    private val audio = context.getSystemService(AudioManager::class.java)
    private val power = context.getSystemService(PowerManager::class.java)
    private val notifications = context.getSystemService(NotificationManager::class.java)
    private val main = Handler(Looper.getMainLooper())

    @Volatile
    var foreground = false

    private val snapshot = TtlValue(TTL_MS, clock) { read() }
    private val motor: Boolean by lazy { vibrator?.hasVibrator() == true }

    private fun read() = SystemSnapshot(
        interactive = power?.isInteractive != false,
        touchHaptics = Settings.System.getInt(context.contentResolver, Settings.System.HAPTIC_FEEDBACK_ENABLED, 1) != 0,
        touchSounds = Settings.System.getInt(context.contentResolver, Settings.System.SOUND_EFFECTS_ENABLED, 1) != 0,
        ringerNormal = audio?.ringerMode.let { it != AudioManager.RINGER_MODE_SILENT && it != AudioManager.RINGER_MODE_VIBRATE },
        dnd = when (notifications?.currentInterruptionFilter) {
            NotificationManager.INTERRUPTION_FILTER_NONE, NotificationManager.INTERRUPTION_FILTER_ALARMS -> true
            else -> false
        },
        streamAudible = audio == null || audio.getStreamVolume(AudioManager.STREAM_SYSTEM) > 0,
    )

    override val active: Boolean get() = foreground && snapshot.get().interactive
    override val systemTouchHaptics: Boolean get() = snapshot.get().touchHaptics
    override val systemTouchSounds: Boolean get() = snapshot.get().touchSounds
    override val ringerNormal: Boolean get() = snapshot.get().ringerNormal
    override val silencedByDnd: Boolean get() = snapshot.get().dnd
    override val hasVibrator: Boolean get() = motor
    override val streamAudible: Boolean get() = snapshot.get().streamAudible

    /** Drop the cache: the Settings page reads the truth, and coming back to the foreground starts fresh. */
    fun refresh() = snapshot.invalidate()

    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) = snapshot.invalidate()
    }
    private val observer = object : ContentObserver(main) {
        override fun onChange(selfChange: Boolean) = snapshot.invalidate()
    }
    private var watching = false

    /** Keep the snapshot exact while the app is in the foreground; costs nothing in the background. */
    fun watch(on: Boolean) {
        if (on == watching) return
        watching = on
        if (on) {
            val filter = IntentFilter().apply {
                addAction(AudioManager.RINGER_MODE_CHANGED_ACTION)
                addAction(VOLUME_CHANGED_ACTION)
                addAction(NotificationManager.ACTION_INTERRUPTION_FILTER_CHANGED)
                addAction(Intent.ACTION_SCREEN_ON)
                addAction(Intent.ACTION_SCREEN_OFF)
            }
            ContextCompat.registerReceiver(context, receiver, filter, ContextCompat.RECEIVER_NOT_EXPORTED)
            for (name in listOf(Settings.System.HAPTIC_FEEDBACK_ENABLED, Settings.System.SOUND_EFFECTS_ENABLED)) {
                context.contentResolver.registerContentObserver(Settings.System.getUriFor(name), false, observer)
            }
            snapshot.invalidate()
        } else {
            runCatching { context.unregisterReceiver(receiver) }
            context.contentResolver.unregisterContentObserver(observer)
        }
    }

    companion object {
        /** Safety net behind the broadcasts: nothing stays stale longer than this. */
        const val TTL_MS = 2_000L

        /** `AudioManager.VOLUME_CHANGED_ACTION` is hidden; the string is stable. */
        private const val VOLUME_CHANGED_ACTION = "android.media.VOLUME_CHANGED_ACTION"
    }
}

/**
 * Haptics and sound for the whole app. See [FeedbackGate] for when an event
 * plays, [HapticPlanner] for how a haptic is felt on this hardware,
 * [CueTable] for the sounds, and `docs/sound-design/android.md` for the
 * design.
 *
 * Debug builds log one line per event to the `ZeronFeedback` tag: what
 * played (and as what) or why it did not.
 */
class AndroidFeedback(
    private val context: Context,
    val store: FeedbackStore,
    private val clock: () -> Long = SystemClock::uptimeMillis,
) : Feedback, TapFeedback {
    private val debug = (context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE) != 0
    private val vibrator: Vibrator? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
        context.getSystemService(VibratorManager::class.java)?.defaultVibrator
    } else {
        @Suppress("DEPRECATION") context.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
    }
    private val env = SystemEnvironment(context, vibrator)
    private val gate = FeedbackGate({ store.current }, env, clock)
    private val claims = ClaimTracker(clock)
    private val bank = SoundBank(context)
    private val main = Handler(Looper.getMainLooper())
    private val warm = WarmPolicy(clock)
    private var warming = false
    private val warmCheck = object : Runnable {
        override fun run() {
            if (warm.wanted()) {
                main.postDelayed(this, warm.remainingMs() + 50)
            } else {
                bank.keepWarm(false)
                warming = false
            }
        }
    }
    private var viewRef = WeakReference<View?>(null)

    private val primitives: Set<Int> by lazy {
        val v = vibrator
        if (v == null || Build.VERSION.SDK_INT < Build.VERSION_CODES.R) emptySet()
        else ALL_PRIMITIVES.filter { v.areAllPrimitivesSupported(it) }.toSet()
    }
    private val amplitudeControl: Boolean by lazy { vibrator?.hasAmplitudeControl() == true }

    init {
        // Decode off the main thread; SoundPool.load itself returns at once, this keeps resource lookups off startup.
        Thread({ bank.load() }, "feedback-load").start()
        if (debug) {
            Log.d(
                TAG,
                "ready: primitives=$primitives amplitudeControl=$amplitudeControl vibrator=${vibrator?.hasVibrator()} sdk=${Build.VERSION.SDK_INT} " +
                    "outputRate=${bank.outputRate} assetRate=${OutputPath.ASSET_RATE} resampled=${OutputPath.needsResampling(bank.outputRate)}",
            )
        }
    }

    /** The window's view, for `performHapticFeedback` (OEM tuned effects). */
    fun attachView(view: View?) {
        viewRef = WeakReference(view)
    }

    /** The app is in the foreground (process lifecycle). Nothing plays otherwise: events go to notifications. */
    fun setForeground(foreground: Boolean) {
        env.foreground = foreground
        env.watch(foreground)
        if (!foreground) {
            warm.stop()
            main.removeCallbacks(warmCheck)
            bank.keepWarm(false)
            warming = false
        }
        if (debug) Log.d(TAG, "foreground=$foreground")
    }

    /**
     * A finger went down somewhere in the app (UI thread, `ACTION_DOWN`). Wakes the sound output now so it is
     * running by the time the click that sounds arrives; see [WarmPolicy].
     */
    fun onTouchDown() {
        if (!env.foreground || !store.current.sounds) return
        warm.touch()
        if (!warming) {
            warming = bank.keepWarm(true)
            if (warming) {
                main.removeCallbacks(warmCheck)
                main.postDelayed(warmCheck, WarmPolicy.IDLE_MS + 50)
            }
        }
    }

    // ── Feedback ───────────────────────────────────────────────────────────

    override fun haptic(haptic: Haptic) {
        claims.claimHaptic()
        play(haptic)
    }

    /** [level] 0..1 shapes the haptics that have a range (EffortStep: crisp tick to heavy thunk; Stretch: harder the further pulled). */
    override fun haptic(haptic: Haptic, level: Float) {
        claims.claimHaptic()
        play(haptic, level = level)
    }

    override fun cue(cue: Cue, step: Int) {
        claims.claimCue()
        play(cue, step)
    }

    /** Sound first, then the haptic: the same synchronous call, and the late-perceived channel goes out first. */
    override fun both(haptic: Haptic, cue: Cue, step: Int) {
        cue(cue, step)
        haptic(haptic)
    }

    override fun defaultTap(heldMs: Long) {
        if (heldMs >= ClaimTracker.LONG_PRESS_MS) return
        val released = clock()
        main.postDelayed({
            claims.quiet() // a press just answered: a popover closing around it stays silent
            // The sound first: the vibrator service is a binder call, and the sound is the part that is
            // perceived late. Both are issued back to back from this one callback.
            if (claims.cueClaimed(released)) log("cue Tap skip default tap: claimed") else play(Cue.Tap, 0)
            if (claims.hapticClaimed(released)) log("haptic Select skip default tap: claimed") else play(Haptic.Select)
        }, ClaimTracker.DEFER_MS)
    }

    override fun quietClose() = claims.quiet()

    override fun cueUnlessRecent(cue: Cue, windowMs: Long) {
        if (claims.cueWithin(windowMs)) log("cue $cue skip: another cue just played") else cue(cue)
    }

    /** The Settings page auditioning a moment: ignores rate limits and category switches, nothing else. */
    fun preview(haptic: Haptic?, cue: Cue?, step: Int = 0) {
        cue?.let { claims.claimCue(); play(it, step, preview = true) }
        haptic?.let { claims.claimHaptic(); play(it, preview = true) }
    }

    /** What the system is doing to this app's feedback right now, for the Settings page (empty when nothing). Reads fresh. */
    fun systemNotes(): List<SystemNote> {
        env.refresh()
        return SystemNotes.build(env, store.current)
    }

    /** How richly this vibration motor renders haptics, for the Settings page. */
    fun hapticTier(): String = when {
        vibrator?.hasVibrator() != true -> "No vibration motor"
        VibrationEffect.Composition.PRIMITIVE_CLICK in primitives && VibrationEffect.Composition.PRIMITIVE_TICK in primitives -> "Rich haptics: precise clicks and ticks"
        amplitudeControl -> "Standard haptics: adjustable vibration"
        else -> "Basic haptics: a simple on/off motor"
    }

    // ── haptics ────────────────────────────────────────────────────────────

    private fun play(haptic: Haptic, preview: Boolean = false, level: Float = 0.5f) {
        when (val d = gate.haptic(haptic, preview)) {
            is Decision.Skip -> log("haptic $haptic skip: ${d.why.label}")
            Decision.Play -> {
                val view = viewRef.get()?.takeIf { it.isAttachedToWindow }
                val strength = store.current.strength
                var plan = HapticPlanner.plan(haptic, strength, caps(view != null), level)
                var ok = perform(plan, view)
                if (!ok && plan is HapticPlan.ViewConstant) {
                    plan = HapticPlanner.plan(haptic, strength, caps(false), level)
                    ok = perform(plan, null)
                }
                log("haptic $haptic${if (haptic in HapticTable.leveled) " level=${"%.2f".format(level)}" else ""} ${if (ok) "play" else "skip"} $plan")
            }
        }
    }

    private fun caps(hasView: Boolean) = HapticCapabilities(Build.VERSION.SDK_INT, primitives, amplitudeControl, hasView)

    private fun perform(plan: HapticPlan, view: View?): Boolean = try {
        when (plan) {
            is HapticPlan.Skip -> false
            is HapticPlan.ViewConstant -> view?.performHapticFeedback(plan.constant) == true
            is HapticPlan.Composition -> vibrate(
                VibrationEffect.startComposition().also { c ->
                    plan.steps.forEach { c.addPrimitive(it.primitive, it.scale, it.delayMs) }
                }.compose(),
            )
            is HapticPlan.Predefined -> vibrate(VibrationEffect.createPredefined(plan.effect))
            is HapticPlan.Waveform -> vibrate(
                if (plan.amplitudes != null) VibrationEffect.createWaveform(plan.timings, plan.amplitudes, -1)
                else VibrationEffect.createWaveform(plan.timings, -1),
            )
        }
    } catch (e: RuntimeException) {
        Log.w(TAG, "haptic failed", e)
        false
    }

    private fun vibrate(effect: VibrationEffect): Boolean {
        val v = vibrator ?: return false
        if (Build.VERSION.SDK_INT >= 33) {
            v.vibrate(effect, VibrationAttributes.createForUsage(VibrationAttributes.USAGE_TOUCH))
        } else {
            @Suppress("DEPRECATION")
            v.vibrate(effect, android.media.AudioAttributes.Builder().setUsage(android.media.AudioAttributes.USAGE_ASSISTANCE_SONIFICATION).build())
        }
        return true
    }

    // ── sound ──────────────────────────────────────────────────────────────

    private fun play(cue: Cue, step: Int, preview: Boolean = false) {
        when (val d = gate.cue(cue, preview)) {
            is Decision.Skip -> log("cue $cue skip: ${d.why.label}")
            Decision.Play -> {
                val spec = CueTable.spec(cue)
                val volume = CueTable.volume(spec, store.current.gain)
                val rate = if (cue == Cue.Detent) DetentLadder.rate(step) else 1f
                val started = bank.play(cue, volume, rate, spec.priority)
                log(
                    if (started) "cue $cue play vol=${"%.2f".format(volume)} rate=${"%.2f".format(rate)}${if (cue == Cue.Detent) " step=$step" else ""}"
                    else "cue $cue skip: ${Skipped.NotLoaded.label}",
                )
            }
        }
    }

    private fun log(message: String) {
        if (debug) Log.d(TAG, message)
    }

    companion object {
        const val TAG = "ZeronFeedback"
        private val ALL_PRIMITIVES = intArrayOf(
            VibrationEffect.Composition.PRIMITIVE_CLICK,
            VibrationEffect.Composition.PRIMITIVE_TICK,
            VibrationEffect.Composition.PRIMITIVE_LOW_TICK,
            VibrationEffect.Composition.PRIMITIVE_THUD,
            VibrationEffect.Composition.PRIMITIVE_SPIN,
            VibrationEffect.Composition.PRIMITIVE_QUICK_RISE,
            VibrationEffect.Composition.PRIMITIVE_SLOW_RISE,
            VibrationEffect.Composition.PRIMITIVE_QUICK_FALL,
        )
    }
}
