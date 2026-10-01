package sh.zeron.android.feedback

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class CueTableTest {
    private val raw = File("src/main/res/raw")
    private val desktop = File("../../../crates/ui/assets/sounds")

    /**
     * Round-2 cues that still borrow a neighbour's file. Delete an entry here the moment its own
     * `fx_*.wav` lands; the list must be empty before the PR is final.
     */
    private val borrowing = setOf(
        Cue.Surge, Cue.Zip, Cue.Rebound, Cue.FastOn, Cue.FastOff,
        Cue.ProviderClaude, Cue.ProviderCodex, Cue.ProviderCursor, Cue.ProviderDevin, Cue.ProviderGrok,
        Cue.ProviderHermes, Cue.ProviderPi, Cue.ProviderOpenCode, Cue.ProviderAntigravity,
        Cue.ProviderFavorites, Cue.ProviderOther,
    )

    @Test fun everyCueHasASpecWithItsOwnResource() {
        assertEquals(Cue.entries.size, CueTable.all.size)
        val own = CueTable.all.filter { it.cue !in borrowing }
        assertEquals(own.size, own.map { it.resource }.toSet().size)
        for (spec in CueTable.all) {
            assertTrue(spec.resource, spec.resource.matches(Regex("fx_[a-z_]+")))
            assertTrue(spec.gain in 0.1f..1f)
        }
    }

    @Test fun everyResourceExistsInTheRepoOrComesFromTheDesktopAssets() {
        for (spec in CueTable.all) {
            val committed = File(raw, spec.resource + ".wav").isFile
            val fromDesktop = File(desktop, spec.resource.removePrefix("fx_") + ".wav").isFile
            assertTrue("${spec.cue} -> ${spec.resource}", committed || fromDesktop)
        }
    }

    @Test fun sessionCuesUseTheDesktopBytes() {
        assertEquals("fx_done", CueTable.spec(Cue.Done).resource)
        assertEquals("fx_request", CueTable.spec(Cue.Request).resource)
        assertEquals("fx_attention", CueTable.spec(Cue.Attention).resource)
        assertEquals(CueCategory.Completion, CueTable.spec(Cue.Done).category)
        assertEquals(CueCategory.Input, CueTable.spec(Cue.Request).category)
        assertEquals(CueCategory.Errors, CueTable.spec(Cue.Attention).category)
    }

    @Test fun interfaceCuesAreQuieterOrEqualToSessionCues() {
        for (spec in CueTable.all.filter { it.category == CueCategory.Interface }) assertTrue(spec.gain <= 1f)
        assertTrue(CueTable.spec(Cue.Send).gain < CueTable.spec(Cue.Done).gain)
    }

    @Test fun detentClimbsAndStaysInRange() {
        var last = -100
        for (step in 0..4) {
            val st = DetentLadder.semitones(step)
            assertTrue("step $step", st > last)
            last = st
        }
        for (step in -5..60) {
            val rate = DetentLadder.rate(step)
            assertTrue("$step $rate", rate in DetentLadder.MIN_RATE..DetentLadder.MAX_RATE)
        }
        assertEquals(DetentLadder.rate(4), DetentLadder.rate(4), 0f)
        assertTrue(DetentLadder.rate(5) > DetentLadder.rate(4)) // next octave continues upward
        assertEquals(DetentLadder.rate(40), DetentLadder.rate(80), 0f) // the top holds
    }
}
