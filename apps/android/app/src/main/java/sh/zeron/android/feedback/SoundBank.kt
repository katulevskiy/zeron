package sh.zeron.android.feedback

import android.content.Context
import android.media.AudioAttributes
import android.media.SoundPool
import android.util.Log
import java.util.concurrent.ConcurrentHashMap

/**
 * Every cue preloaded into one [SoundPool]: low latency, no audio focus (UI
 * sounds mix with the user's music), on the sonification usage so they follow
 * the system sound volume and silent mode.
 */
class SoundBank(private val context: Context) {
    private val pool: SoundPool = SoundPool.Builder()
        .setMaxStreams(FeedbackGate.MAX_STREAMS)
        .setAudioAttributes(
            AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_ASSISTANCE_SONIFICATION)
                .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                .build(),
        )
        .build()
    private val ids = ConcurrentHashMap<Cue, Int>()
    private val ready = ConcurrentHashMap.newKeySet<Int>()

    init {
        pool.setOnLoadCompleteListener { _, sampleId, status ->
            if (status == 0) { ready.add(sampleId); Log.d(AndroidFeedback.TAG, "sound $sampleId loaded") } else Log.w(AndroidFeedback.TAG, "sound $sampleId failed to load: $status")
        }
    }

    /** Starts decoding every cue; returns at once (SoundPool loads asynchronously). */
    fun load() {
        for (spec in CueTable.all) {
            val res = context.resources.getIdentifier(spec.resource, "raw", context.packageName)
            if (res != 0) ids[spec.cue] = pool.load(context, res, 1) else Log.w(AndroidFeedback.TAG, "no resource ${spec.resource} for ${spec.cue}")
        }
    }

    fun isReady(cue: Cue): Boolean = ids[cue]?.let { it in ready } == true

    /** [rate] resamples (pitch); [volume] is 0..1. Returns whether a stream started. */
    fun play(cue: Cue, volume: Float, rate: Float, priority: Int): Boolean {
        val id = ids[cue]?.takeIf { it in ready } ?: return false
        return pool.play(id, volume, volume, priority, 0, rate) != 0
    }

    fun release() = pool.release()
}
