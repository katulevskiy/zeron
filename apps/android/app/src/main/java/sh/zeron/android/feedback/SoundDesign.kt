package sh.zeron.android.feedback

import kotlin.math.pow

/**
 * One sound cue as the engine plays it: which file, how loud relative to the
 * set, how important it is when sounds compete, and which in-app switch
 * governs it. Files live in `res/raw` as `fx_<name>.wav`: the interface
 * cues and promoted desktop auditions are committed (see
 * `scripts/generate-android-sounds.py`), and the desktop's `done` /
 * `request` / `attention` are copied from `crates/ui/assets/sounds` by Gradle.
 */
data class CueSpec(
    val cue: Cue,
    /** `res/raw` resource name, without extension. */
    val resource: String,
    val category: CueCategory,
    /** Level trim against the rest of the set, 0..1 (before the user's volume). */
    val gain: Float,
    /** 0 interface, 1 action feedback, 2 session event, 3 alert. Higher wins when streams are scarce. */
    val priority: Int,
    /** The same cue is never stacked within this many ms. */
    val minGapMs: Long = 60,
    /** Approximate length, used to count streams still sounding. */
    val approxMs: Long = 120,
)

object CueTable {
    fun spec(cue: Cue): CueSpec = when (cue) {
        // Session events: the desktop's own chimes at their native level (2-4 dB above the interface set).
        Cue.Done -> CueSpec(cue, "fx_done", CueCategory.Completion, 1f, 2, 400, 560)
        Cue.Request -> CueSpec(cue, "fx_request", CueCategory.Input, 1f, 2, 400, 600)
        Cue.Attention -> CueSpec(cue, "fx_attention", CueCategory.Errors, 1f, 3, 400, 660)

        // Desktop audition cues promoted for the phone (about 2 dB hotter than the interface set, so trimmed).
        Cue.Send -> CueSpec(cue, "fx_send", CueCategory.Interface, 0.8f, 1, 120, 160)
        Cue.Queued -> CueSpec(cue, "fx_queued", CueCategory.Interface, 0.8f, 1, 120, 200)
        Cue.UploadReady -> CueSpec(cue, "fx_upload_ready", CueCategory.Interface, 0.8f, 2, 250, 300)
        Cue.Reconnected -> CueSpec(cue, "fx_reconnected", CueCategory.Errors, 0.8f, 2, 400, 450)
        Cue.Undo -> CueSpec(cue, "fx_undo", CueCategory.Interface, 0.8f, 1, 120, 250)

        // Interface cues: subliminal, matched in loudness by the audit.
        Cue.Tap -> CueSpec(cue, "fx_tap", CueCategory.Interface, 1f, 0, 60, 60)
        Cue.Select -> CueSpec(cue, "fx_select", CueCategory.Interface, 1f, 0, 60, 80)
        Cue.ToggleOn -> CueSpec(cue, "fx_toggle_on", CueCategory.Interface, 1f, 1, 80, 90)
        Cue.ToggleOff -> CueSpec(cue, "fx_toggle_off", CueCategory.Interface, 1f, 1, 80, 90)
        Cue.Open -> CueSpec(cue, "fx_open", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Close -> CueSpec(cue, "fx_close", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Detent -> CueSpec(cue, "fx_detent", CueCategory.Interface, 1f, 0, 60, 60)
        Cue.Star -> CueSpec(cue, "fx_star", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Unstar -> CueSpec(cue, "fx_unstar", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Pin -> CueSpec(cue, "fx_pin", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Archive -> CueSpec(cue, "fx_archive", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Delete -> CueSpec(cue, "fx_delete", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Copy -> CueSpec(cue, "fx_copy", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Error -> CueSpec(cue, "fx_error", CueCategory.Interface, 1f, 3, 250, 180)
        Cue.Refresh -> CueSpec(cue, "fx_refresh", CueCategory.Interface, 1f, 1, 200, 120)

        // Round 2 placeholders (real assets replace these): borrow a neighbour's file.
        Cue.Surge, Cue.FastOn -> spec(Cue.Done).copy(cue = cue, category = CueCategory.Interface, priority = 1)
        Cue.Zip, Cue.Rebound, Cue.FastOff -> spec(Cue.Select).copy(cue = cue)
        Cue.ProviderClaude, Cue.ProviderCodex, Cue.ProviderCursor, Cue.ProviderDevin, Cue.ProviderGrok,
        Cue.ProviderHermes, Cue.ProviderPi, Cue.ProviderOpenCode, Cue.ProviderAntigravity,
        Cue.ProviderFavorites, Cue.ProviderOther -> spec(Cue.Select).copy(cue = cue)
    }

    val all: List<CueSpec> get() = Cue.entries.map(::spec)
}

/**
 * The Detent cue climbs a major-pentatonic ladder (the family's key), played
 * by resampling one 880 Hz tick: step 0 sits a fourth below the reference
 * and each step walks up the scale, so dragging a slider "plays" it. Rates are
 * kept inside SoundPool's 0.5..2.0 range; beyond the top the ladder holds.
 */
object DetentLadder {
    private val degrees = intArrayOf(0, 2, 4, 7, 9)
    const val BASE_SEMITONES = -7
    const val MAX_SEMITONES = 12
    const val MIN_RATE = 0.5f
    const val MAX_RATE = 2.0f

    fun semitones(step: Int): Int {
        val octave = Math.floorDiv(step, degrees.size)
        val degree = Math.floorMod(step, degrees.size)
        return (BASE_SEMITONES + 12 * octave + degrees[degree]).coerceIn(-12, MAX_SEMITONES)
    }

    fun rate(step: Int): Float = 2.0.pow(semitones(step) / 12.0).toFloat().coerceIn(MIN_RATE, MAX_RATE)
}
