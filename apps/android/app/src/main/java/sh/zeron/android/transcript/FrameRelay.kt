package sh.zeron.android.transcript

import uniffi.zeron_core.LayoutListener

/**
 * Breaks the construction cycle between the Rust layout view (which needs a
 * listener up front) and [TranscriptCanvas] (which needs the view): the
 * relay is handed to `TranscriptView(…)`, the canvas's listener plugged in
 * after. Called on the Rust layout thread.
 */
class FrameRelay : LayoutListener {
    @Volatile var target: LayoutListener? = null

    override fun frameReady(revision: ULong) {
        target?.frameReady(revision)
    }
}
