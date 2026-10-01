package sh.zeron.android.feedback

import android.app.NotificationManager
import android.content.Context
import android.content.pm.ApplicationInfo
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
import java.lang.ref.WeakReference

/** The one place the rest of the app reaches the engine without Compose (view models, `AppModel`, tools). */
object AppFeedback {
    @Volatile
    var current: Feedback = NoFeedback
}

/** The real [FeedbackEnvironment]: the activity lifecycle plus the system's sound, vibration and DND state. */
class SystemEnvironment(private val context: Context, val vibrator: Vibrator?) : FeedbackEnvironment {
    private val audio = context.getSystemService(AudioManager::class.java)
    private val power = context.getSystemService(PowerManager::class.java)
    private val notifications = context.getSystemService(NotificationManager::class.java)

    @Volatile
    var foreground = false

    override val active: Boolean get() = foreground && power?.isInteractive != false
    override val systemTouchHaptics: Boolean
        get() = Settings.System.getInt(context.contentResolver, Settings.System.HAPTIC_FEEDBACK_ENABLED, 1) != 0
    override val systemTouchSounds: Boolean
        get() = Settings.System.getInt(context.contentResolver, Settings.System.SOUND_EFFECTS_ENABLED, 1) != 0
    override val ringerNormal: Boolean get() = audio?.ringerMode != AudioManager.RINGER_MODE_SILENT && audio?.ringerMode != AudioManager.RINGER_MODE_VIBRATE
    override val silencedByDnd: Boolean
        get() = when (notifications?.currentInterruptionFilter) {
            NotificationManager.INTERRUPTION_FILTER_NONE, NotificationManager.INTERRUPTION_FILTER_ALARMS -> true
            else -> false
        }
    override val hasVibrator: Boolean get() = vibrator?.hasVibrator() == true
    override val streamAudible: Boolean get() = audio == null || audio.getStreamVolume(AudioManager.STREAM_SYSTEM) > 0
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
        if (debug) Log.d(TAG, "ready: primitives=$primitives amplitudeControl=$amplitudeControl vibrator=${vibrator?.hasVibrator()} sdk=${Build.VERSION.SDK_INT}")
    }

    /** The window's view, for `performHapticFeedback` (OEM tuned effects). */
    fun attachView(view: View?) {
        viewRef = WeakReference(view)
    }

    /** The app is in the foreground (process lifecycle). Nothing plays otherwise: events go to notifications. */
    fun setForeground(foreground: Boolean) {
        env.foreground = foreground
        if (debug) Log.d(TAG, "foreground=$foreground")
    }

    // ── Feedback ───────────────────────────────────────────────────────────

    override fun haptic(haptic: Haptic) {
        claims.claimHaptic()
        play(haptic)
    }

    override fun cue(cue: Cue, step: Int) {
        claims.claimCue()
        play(cue, step)
    }

    override fun defaultTap(heldMs: Long) {
        if (heldMs >= ClaimTracker.LONG_PRESS_MS) return
        val released = clock()
        main.postDelayed({
            if (claims.hapticClaimed(released)) log("haptic Select skip default tap: claimed") else play(Haptic.Select)
            if (claims.cueClaimed(released)) log("cue Tap skip default tap: claimed") else play(Cue.Tap, 0)
        }, ClaimTracker.DEFER_MS)
    }

    override fun cueUnlessRecent(cue: Cue, windowMs: Long) {
        if (claims.cueWithin(windowMs)) log("cue $cue skip: another cue just played") else cue(cue)
    }

    /** The Settings page auditioning a moment: ignores rate limits and category switches, nothing else. */
    fun preview(haptic: Haptic?, cue: Cue?, step: Int = 0) {
        haptic?.let { claims.claimHaptic(); play(it, preview = true) }
        cue?.let { claims.claimCue(); play(it, step, preview = true) }
    }

    /** What the system is doing to this app's feedback right now, for the Settings page (empty when nothing). */
    fun systemNotes(): List<String> = buildList {
        if (!env.hasVibrator) add("This device has no vibration motor, so haptics are off.")
        else if (!env.systemTouchHaptics) add("Touch vibration is turned off in system settings, so haptics are off.")
        if (!env.ringerNormal) add("The phone is on silent or vibrate, so sounds are muted.")
        else if (env.silencedByDnd) add("Do Not Disturb is silencing sounds.")
        else if (!env.streamAudible) add("The system sound volume is at zero.")
        else if (!env.systemTouchSounds) add("Touch sounds are turned off in system settings, so interface sounds are muted.")
    }

    /** How richly this vibration motor renders haptics, for the Settings page. */
    fun hapticTier(): String = when {
        vibrator?.hasVibrator() != true -> "No vibration motor"
        VibrationEffect.Composition.PRIMITIVE_CLICK in primitives && VibrationEffect.Composition.PRIMITIVE_TICK in primitives -> "Rich haptics: precise clicks and ticks"
        amplitudeControl -> "Standard haptics: adjustable vibration"
        else -> "Basic haptics: a simple on/off motor"
    }

    // ── haptics ────────────────────────────────────────────────────────────

    private fun play(haptic: Haptic, preview: Boolean = false) {
        when (val d = gate.haptic(haptic, preview)) {
            is Decision.Skip -> log("haptic $haptic skip: ${d.why.label}")
            Decision.Play -> {
                val view = viewRef.get()?.takeIf { it.isAttachedToWindow }
                val strength = store.current.strength
                var plan = HapticPlanner.plan(haptic, strength, caps(view != null))
                var ok = perform(plan, view)
                if (!ok && plan is HapticPlan.ViewConstant) {
                    plan = HapticPlanner.plan(haptic, strength, caps(false))
                    ok = perform(plan, null)
                }
                log("haptic $haptic ${if (ok) "play" else "skip"} $plan")
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
                val volume = (store.current.gain * spec.gain).coerceIn(0f, 1f)
                val rate = if (cue == Cue.Detent) DetentLadder.rate(step) else 1f
                val started = bank.play(cue, volume, rate, spec.priority)
                log("cue $cue ${if (started) "play" else "skip: ${Skipped.NotLoaded.label}"} vol=${"%.2f".format(volume)} rate=${"%.2f".format(rate)}${if (cue == Cue.Detent) " step=$step" else ""}")
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
