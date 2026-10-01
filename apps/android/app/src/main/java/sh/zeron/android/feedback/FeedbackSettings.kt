package sh.zeron.android.feedback

import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** How hard haptics hit: scales composition primitives (and waveform amplitudes) where the hardware allows. */
enum class HapticStrength(val label: String, val scale: Float) {
    Subtle("Subtle", 0.5f),
    Standard("Standard", 1f),
    Strong("Strong", 1.5f),
}

/**
 * Which in-app switch governs a sound. Mirrors the desktop's Notifications
 * settings: one master, independent completion / input / error chimes, plus
 * the phone's own interface sounds.
 */
enum class CueCategory {
    /** Taps, toggles, sheets, detents: the quiet interface layer. */
    Interface,

    /** A turn finished. */
    Completion,

    /** A question or approval is waiting. */
    Input,

    /** A run failed or the connection dropped (and came back). */
    Errors,
}

/** The user's choices for sound and haptics. Everything defaults on, at restrained levels. */
data class FeedbackSettings(
    /** Master switch for every sound. */
    val sounds: Boolean = true,
    val interfaceSounds: Boolean = true,
    /** Master for the three session chimes below. */
    val sessionSounds: Boolean = true,
    val completionSound: Boolean = true,
    val inputSound: Boolean = true,
    val errorSound: Boolean = true,
    /** 0..1 slider; the audible gain follows [gain]. 50% is the loudness the app always had, 100% is twice that. */
    val volume: Float = DEFAULT_VOLUME,
    val haptics: Boolean = true,
    val strength: HapticStrength = HapticStrength.Standard,
    /**
     * Play interface sounds even though the phone's "Touch sounds" setting is off. The cues are played as
     * sonification through [android.media.SoundPool], not through the system's touch-sound path, so the flag
     * does not technically block them: Zeron honours it as a courtesy unless this is on.
     */
    val ignoreSystemTouchSounds: Boolean = false,
) {
    fun allows(category: CueCategory): Boolean = sounds && when (category) {
        CueCategory.Interface -> interfaceSounds
        CueCategory.Completion -> sessionSounds && completionSound
        CueCategory.Input -> sessionSounds && inputSound
        CueCategory.Errors -> sessionSounds && errorSound
    }

    /** Linear gain on the cue's native level, 0..[MAX_GAIN]; see [gainFor]. */
    val gain: Float get() = gainFor(volume)

    companion object {
        /** Half way: the loudness the app had before the slider was widened to double it. */
        const val DEFAULT_VOLUME = 0.5f

        /** At 100% every cue is twice as loud (+6 dB) as at the default 50%. */
        const val MAX_GAIN = 2f

        /**
         * The slider is linear, loudness is not. The slider position doubles to `v' = 2 * slider` and goes through
         * the curve the app always used up to its old maximum (v' <= 1: gain = v'^2, so 25% of the old slider reads
         * about -12 dB), then continues in equal dB steps to +6 dB at the new maximum (gain = 2^(v' - 1)). Both
         * pieces meet at gain 1 with no jump, and the default (50%) is exactly the old full volume.
         */
        fun gainFor(slider: Float): Float {
            val v = 2f * slider.coerceIn(0f, 1f)
            return if (v <= 1f) v * v else Math.pow(2.0, (v - 1f).toDouble()).toFloat()
        }
    }
}

/** Plain key/value form of [FeedbackSettings], kept free of Android types so it can be tested. */
object FeedbackSettingsCodec {
    private const val P = "feedback."

    fun write(s: FeedbackSettings): Map<String, Any> = mapOf(
        P + "sounds" to s.sounds,
        P + "interface" to s.interfaceSounds,
        P + "session" to s.sessionSounds,
        P + "completion" to s.completionSound,
        P + "input" to s.inputSound,
        P + "errors" to s.errorSound,
        P + "volume" to s.volume,
        P + "haptics" to s.haptics,
        P + "strength" to s.strength.name,
        P + "ignore_touch_sounds" to s.ignoreSystemTouchSounds,
        VERSION_KEY to VERSION,
    )

    /** Missing or malformed values read as the defaults. */
    fun read(get: (String) -> Any?): FeedbackSettings {
        val d = FeedbackSettings()
        fun bool(key: String, default: Boolean) = get(P + key) as? Boolean ?: default
        return FeedbackSettings(
            sounds = bool("sounds", d.sounds),
            interfaceSounds = bool("interface", d.interfaceSounds),
            sessionSounds = bool("session", d.sessionSounds),
            completionSound = bool("completion", d.completionSound),
            inputSound = bool("input", d.inputSound),
            errorSound = bool("errors", d.errorSound),
            volume = (get(P + "volume") as? Float ?: d.volume).takeIf { it.isFinite() }?.coerceIn(0f, 1f) ?: d.volume,
            haptics = bool("haptics", d.haptics),
            strength = HapticStrength.entries.firstOrNull { it.name == get(P + "strength") } ?: d.strength,
            ignoreSystemTouchSounds = bool("ignore_touch_sounds", d.ignoreSystemTouchSounds),
        )
    }

    /** Layout of the stored values; 1 (absent) had the volume on the old 0..100% scale whose maximum is now 50%. */
    const val VERSION = 2
    private const val VERSION_KEY = P + "prefs_version"

    /**
     * What to write so stored values from an older layout keep their meaning. Version 1 stored the volume on the
     * old scale (100% = the loudness now at 50%), so the same loudness is half the number on the new slider
     * (`gain(v / 2)` on the new curve is `v^2`, exactly the old gain). A fresh install has no volume and just gets
     * stamped. Empty when already current.
     */
    fun migrate(get: (String) -> Any?): Map<String, Any> {
        val stored = (get(VERSION_KEY) as? Int) ?: 1
        if (stored >= VERSION) return emptyMap()
        val out = HashMap<String, Any>()
        (get(P + "volume") as? Float)?.takeIf { it.isFinite() }?.let { out[P + "volume"] = it.coerceIn(0f, 1f) / 2f }
        out[VERSION_KEY] = VERSION
        return out
    }
}

/** [FeedbackSettings] persisted in the app's `settings` preferences. */
class FeedbackStore(private val prefs: SharedPreferences) {
    init {
        val updates = FeedbackSettingsCodec.migrate { prefs.all[it] }
        if (updates.isNotEmpty()) save(prefs.edit(), updates)
    }

    private val _settings = MutableStateFlow(FeedbackSettingsCodec.read { prefs.all[it] })
    val settings: StateFlow<FeedbackSettings> = _settings.asStateFlow()

    val current: FeedbackSettings get() = _settings.value

    fun update(change: FeedbackSettings.() -> FeedbackSettings) {
        val next = _settings.value.change()
        if (next == _settings.value) return
        _settings.value = next
        save(prefs.edit(), FeedbackSettingsCodec.write(next))
    }

    private fun save(edit: SharedPreferences.Editor, values: Map<String, Any>) {
        for ((key, value) in values) {
            when (value) {
                is Boolean -> edit.putBoolean(key, value)
                is Float -> edit.putFloat(key, value)
                is Int -> edit.putInt(key, value)
                is String -> edit.putString(key, value)
            }
        }
        edit.apply()
    }
}
