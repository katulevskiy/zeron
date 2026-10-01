package sh.zeron.android.feedback

/** What the device and the system are doing right now. Read per event; cheap. */
interface FeedbackEnvironment {
    /** The app is in the foreground and its screen is on and interactive. */
    val active: Boolean

    /** System Settings > touch feedback / vibration (`HAPTIC_FEEDBACK_ENABLED`). */
    val systemTouchHaptics: Boolean

    /** System Settings > touch sounds (`SOUND_EFFECTS_ENABLED`). */
    val systemTouchSounds: Boolean

    /** The ringer is normal (not silent or vibrate-only). */
    val ringerNormal: Boolean

    /** Do Not Disturb is on in a mode that silences interface sounds (total silence or alarms only). */
    val silencedByDnd: Boolean

    val hasVibrator: Boolean

    /** The sound-effects stream is audible (volume above zero). */
    val streamAudible: Boolean
}

/** Why a haptic or cue did not play. Logged by debug builds. */
enum class Skipped(val label: String) {
    HapticsOff("haptics switch off"),
    NoVibrator("no vibrator"),
    Inactive("app not active (background or screen off)"),
    SystemHapticsOff("system touch feedback off"),
    SoundsOff("sounds switch off"),
    CategoryOff("category switch off"),
    SystemSoundsOff("system touch sounds off"),
    Silent("ringer silent or vibrate"),
    Dnd("do not disturb"),
    Muted("sound stream at zero"),
    Rate("rate limited"),
    Streams("too many streams"),
    NotLoaded("sound not loaded"),
}

sealed interface Decision {
    data object Play : Decision
    data class Skip(val why: Skipped) : Decision
}

/**
 * Decides whether a haptic or cue plays: the in-app switches, the system's
 * own touch settings, whether the app is active, and rate limiting. Pure and
 * clock-injected so its rules are unit tested; the engine acts on the result.
 *
 * Rules, in order:
 *  - Haptics: the Haptics switch, a vibrator, an active app, and the system
 *    touch-feedback setting. The ringer mode does not matter (Android's own
 *    keyboard and touch haptics keep working on silent).
 *  - Sounds: the Sounds master and the category switch, an active app, the
 *    ringer (silent and vibrate mute), Do Not Disturb (total silence / alarms
 *    only), a non-zero stream volume, and for interface cues the system's
 *    touch-sounds setting. Session chimes ignore the touch-sounds setting
 *    (they are events, not touch feedback) but still follow every other rule.
 *  - Rate: the same haptic / cue never repeats inside its minimum gap, any
 *    two light events keep a short global gap, a burst of light events is
 *    capped per second, and sound streams are capped.
 */
class FeedbackGate(
    private val settings: () -> FeedbackSettings,
    private val env: FeedbackEnvironment,
    private val clock: () -> Long,
) {
    private val lastHaptic = HashMap<Haptic, Long>()
    private var lastAnyHaptic = Long.MIN_VALUE / 2
    private var lastAnyHapticPriority = 0
    private val lightHaptics = ArrayDeque<Long>()

    private val lastCue = HashMap<Cue, Long>()
    private var lastAnyCue = Long.MIN_VALUE / 2
    private var lastAnyCuePriority = 0
    private val streams = ArrayDeque<Pair<Long, Int>>() // end time, priority

    /** [preview]: the Settings page auditioning a feedback: the rate limits do not apply (a tap on a row is intent). */
    @Synchronized
    fun haptic(haptic: Haptic, preview: Boolean = false): Decision {
        val s = settings()
        if (!s.haptics) return skip(Skipped.HapticsOff)
        if (!env.hasVibrator) return skip(Skipped.NoVibrator)
        if (!env.active) return skip(Skipped.Inactive)
        if (!env.systemTouchHaptics) return skip(Skipped.SystemHapticsOff)
        if (preview) return Decision.Play
        val spec = HapticTable.spec(haptic)
        val now = clock()
        if (now - (lastHaptic[haptic] ?: Long.MIN_VALUE / 2) < spec.minGapMs) return skip(Skipped.Rate)
        if (spec.priority <= lastAnyHapticPriority && now - lastAnyHaptic < GLOBAL_HAPTIC_GAP_MS) return skip(Skipped.Rate)
        if (spec.priority == 0) {
            while (lightHaptics.isNotEmpty() && now - lightHaptics.first() > WINDOW_MS) lightHaptics.removeFirst()
            if (lightHaptics.size >= LIGHT_HAPTICS_PER_WINDOW) return skip(Skipped.Rate)
            lightHaptics.addLast(now)
        }
        lastHaptic[haptic] = now
        lastAnyHaptic = now
        lastAnyHapticPriority = spec.priority
        return Decision.Play
    }

    /** [preview]: as for [haptic], and the category switches do not apply (hearing a muted category's sound is the point of the row). */
    @Synchronized
    fun cue(cue: Cue, preview: Boolean = false): Decision {
        val s = settings()
        val spec = CueTable.spec(cue)
        if (!s.sounds) return skip(Skipped.SoundsOff)
        if (!preview && !s.allows(spec.category)) return skip(Skipped.CategoryOff)
        if (!env.active) return skip(Skipped.Inactive)
        if (!env.ringerNormal) return skip(Skipped.Silent)
        if (env.silencedByDnd) return skip(Skipped.Dnd)
        if (!env.streamAudible) return skip(Skipped.Muted)
        if (spec.category == CueCategory.Interface && !env.systemTouchSounds) return skip(Skipped.SystemSoundsOff)
        if (preview) return Decision.Play
        val now = clock()
        if (now - (lastCue[cue] ?: Long.MIN_VALUE / 2) < spec.minGapMs) return skip(Skipped.Rate)
        if (spec.priority <= lastAnyCuePriority && now - lastAnyCue < GLOBAL_CUE_GAP_MS) return skip(Skipped.Rate)
        while (streams.isNotEmpty() && streams.first().first <= now) streams.removeFirst()
        if (streams.size >= MAX_STREAMS && streams.minOf { it.second } >= spec.priority) return skip(Skipped.Streams)
        streams.addLast(now + spec.approxMs to spec.priority)
        lastCue[cue] = now
        lastAnyCue = now
        lastAnyCuePriority = spec.priority
        return Decision.Play
    }

    private fun skip(why: Skipped): Decision = Decision.Skip(why)

    companion object {
        const val GLOBAL_HAPTIC_GAP_MS = 25L
        const val GLOBAL_CUE_GAP_MS = 20L
        const val WINDOW_MS = 1000L
        const val LIGHT_HAPTICS_PER_WINDOW = 12
        const val MAX_STREAMS = 4
    }
}
