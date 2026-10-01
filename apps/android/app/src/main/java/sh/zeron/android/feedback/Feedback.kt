package sh.zeron.android.feedback

import androidx.compose.runtime.staticCompositionLocalOf

/**
 * The app's touch vocabulary. Names describe the *moment*, not the vibration:
 * the engine behind [Feedback] decides how each one is felt on this device
 * (platform effects, composition primitives, or nothing at all).
 */
enum class Haptic {
    /** A light detent: slider steps, scrubbing through discrete items. */
    Tick,

    /** A choice was made: a chip, tab, menu item or list row. */
    Select,

    /** A switch or checkbox turned on / off. */
    ToggleOn,
    ToggleOff,

    /** A press that begins something: press-and-hold, a drag picked up. */
    Press,

    /** A user action was committed: send, create, save, start. */
    Confirm,

    /** Something finished well: a turn completed, a transfer arrived. */
    Success,

    /** Something needs the user: a question, an approval. */
    Attention,

    /** Something failed or was refused. */
    Error,

    /** A long press was recognised: a menu or selection started. */
    LongPress,

    /** A drag or swipe crossed its commit threshold. */
    Threshold,

    /** A weighty, destructive step: delete, uninstall, discard. */
    Heavy,

    /** A small pop: starred, pinned. */
    Pop,
}

/**
 * Sound cues. The first group reuses the desktop's notification family
 * (`docs/sound-design`); the rest are interface cues synthesised in the same
 * "rounded pressure pulse" style (`scripts/generate-android-sounds.py`).
 */
enum class Cue {
    // Session events (desktop assets: done.wav, request.wav, attention.wav).
    Done,
    Request,
    Attention,

    // Desktop audition cues promoted for the phone.
    Send,
    Queued,
    UploadReady,
    Reconnected,
    Undo,

    // Interface.
    Tap,
    Select,
    ToggleOn,
    ToggleOff,
    Open,
    Close,

    /** A slider / stepper detent; `step` raises the pitch a little per position. */
    Detent,
    Star,
    Unstar,
    Pin,
    Archive,
    Delete,
    Copy,
    Error,
    Refresh,
}

interface Feedback {
    fun haptic(haptic: Haptic)

    /** [step] only matters to cues that climb a scale (see [Cue.Detent]). */
    fun cue(cue: Cue, step: Int = 0)

    /** A haptic and a sound for the same moment, the common case. */
    fun both(haptic: Haptic, cue: Cue, step: Int = 0) {
        haptic(haptic)
        cue(cue, step)
    }
}

/** Does nothing: previews, tests, and the default until the app installs the real one. */
object NoFeedback : Feedback {
    override fun haptic(haptic: Haptic) = Unit
    override fun cue(cue: Cue, step: Int) = Unit
}

val LocalFeedback = staticCompositionLocalOf<Feedback> { NoFeedback }
