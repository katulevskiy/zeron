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
    /** 0..1 slider; the audible gain follows [gain]. */
    val volume: Float = DEFAULT_VOLUME,
    val haptics: Boolean = true,
    val strength: HapticStrength = HapticStrength.Standard,
) {
    fun allows(category: CueCategory): Boolean = sounds && when (category) {
        CueCategory.Interface -> interfaceSounds
        CueCategory.Completion -> sessionSounds && completionSound
        CueCategory.Input -> sessionSounds && inputSound
        CueCategory.Errors -> sessionSounds && errorSound
    }

    /** Perceptual curve: the slider is linear, loudness is not (50% reads about -12 dB). */
    val gain: Float get() = volume.coerceIn(0f, 1f).let { it * it }

    companion object {
        const val DEFAULT_VOLUME = 1f
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
        )
    }
}

/** [FeedbackSettings] persisted in the app's `settings` preferences. */
class FeedbackStore(private val prefs: SharedPreferences) {
    private val _settings = MutableStateFlow(FeedbackSettingsCodec.read { prefs.all[it] })
    val settings: StateFlow<FeedbackSettings> = _settings.asStateFlow()

    val current: FeedbackSettings get() = _settings.value

    fun update(change: FeedbackSettings.() -> FeedbackSettings) {
        val next = _settings.value.change()
        if (next == _settings.value) return
        _settings.value = next
        val edit = prefs.edit()
        for ((key, value) in FeedbackSettingsCodec.write(next)) {
            when (value) {
                is Boolean -> edit.putBoolean(key, value)
                is Float -> edit.putFloat(key, value)
                is String -> edit.putString(key, value)
            }
        }
        edit.apply()
    }
}
