package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import sh.zeron.android.core.FavoriteModel
import uniffi.zeron_core.ModelInfo

private fun choice(harness: String, id: String, label: String = harness) =
    ModelChoice(harness, label, ModelInfo(id, id, null, emptyList(), emptyList(), null))

private fun entries(vararg pairs: Pair<String, String>, starred: Set<String> = emptySet()) =
    pairs.map { (h, id) -> ModelPickerRules.Entry(choice(h, id, h.uppercase()), starred = id in starred) }

class RailRulesTest {
    private val catalog = entries("claude-code" to "opus", "claude-code" to "sonnet", "codex" to "gpt", "mystery" to "m1", "pi" to "p1")

    @Test fun providersAreCanonicalAndUnknownOnesAreOther() {
        assertEquals("claude-code", RailRules.providerOf("claude-code"))
        assertEquals("pi", RailRules.providerOf("pi"))
        assertEquals(RailRules.OTHER, RailRules.providerOf("mystery"))
        assertEquals(RailRules.OTHER, RailRules.providerOf("mock"))
    }

    @Test fun sectionsFollowCatalogOrderWithFavoritesFirst() {
        val e = entries("codex" to "gpt", "claude-code" to "opus", "claude-code" to "sonnet", starred = setOf("opus"))
        // The list already puts starred first; the layout keeps what it is given.
        val ordered = e.sortedBy { !it.starred }
        val layout = RailRules.layout(ordered, headers = true)
        assertEquals(listOf("favorites", "codex", "claude-code"), layout.sections.map { it.id })
        assertEquals(listOf("Favorites", "CODEX", "CLAUDE-CODE"), layout.sections.map { it.label })
        // Every section opens with its header; items = headers + rows.
        assertEquals(3 + 3, layout.items.size)
        assertEquals(listOf(0, 2, 4), layout.sections.map { it.item })
        assertTrue(layout.items[0] is RailRules.Item.Header)
    }

    @Test fun unknownProvidersGatherUnderOtherEvenWhenInterleaved() {
        val e = entries("zed" to "a", "codex" to "gpt", "yak" to "b")
        val layout = RailRules.layout(e, headers = true)
        assertEquals(listOf("other", "codex"), layout.sections.map { it.id })
        // Rows are grouped so each section is contiguous: other, a, b, codex, gpt.
        val keys = layout.items.map { it.key }
        assertEquals(listOf("section/other", "zed/a", "yak/b", "section/codex", "codex/gpt"), keys)
    }

    @Test fun withoutHeadersTheOrderIsUntouched() {
        val layout = RailRules.layout(catalog, headers = false)
        assertEquals(catalog.map { it.key }, layout.items.map { it.key })
        assertEquals(listOf("claude-code", "codex", "other", "pi"), layout.sections.map { it.id })
    }

    @Test fun railNeedsTwoSectionsAndNoSearch() {
        assertTrue(RailRules.visible(catalog, ""))
        assertFalse(RailRules.visible(catalog, "gpt"))
        assertFalse(RailRules.visible(entries("codex" to "a", "codex" to "b"), ""))
        assertFalse(RailRules.visible(emptyList(), ""))
    }

    @Test fun theLitSectionIsTheOneAtTheTopOfTheList() {
        val layout = RailRules.layout(catalog, headers = true)
        // items: [hdr claude, opus, sonnet, hdr codex, gpt, hdr other, m1, hdr pi, p1]
        assertEquals(0, RailRules.current(layout, 0, canScrollForward = true))
        assertEquals(0, RailRules.current(layout, 2, canScrollForward = true))
        assertEquals(1, RailRules.current(layout, 3, canScrollForward = true))
        assertEquals(2, RailRules.current(layout, 6, canScrollForward = true))
        assertEquals(3, RailRules.current(layout, 8, canScrollForward = true))
    }

    @Test fun atTheEndOfTheListTheLastSectionLights() {
        val layout = RailRules.layout(catalog, headers = true)
        // The last section is short and never reaches the top: the list's end counts as arriving at it.
        assertEquals(3, RailRules.current(layout, 5, canScrollForward = false))
        // A list that doesn't scroll at all stays on its first section.
        assertEquals(0, RailRules.current(layout, 0, canScrollForward = false))
    }

    @Test fun scrubbingMapsAFingerToASlotAndClamps() {
        assertEquals(0, RailRules.slotAt(-30f, 30f, 0f, 4))
        assertEquals(0, RailRules.slotAt(29f, 30f, 0f, 4))
        assertEquals(1, RailRules.slotAt(30f, 30f, 0f, 4))
        assertEquals(3, RailRules.slotAt(500f, 30f, 0f, 4))
        assertEquals(0, RailRules.slotAt(10f, 0f, 0f, 4))
        assertEquals(0, RailRules.slotAt(10f, 30f, 0f, 0))
    }

    @Test fun slotsShrinkToFitButNotBelowAFingerFriendlyMinimum() {
        assertEquals(36f, RailRules.slotHeight(400f, 3, 26f, 36f), 0f)
        assertEquals(30f, RailRules.slotHeight(150f, 5, 26f, 36f), 0f)
        assertEquals(26f, RailRules.slotHeight(100f, 8, 26f, 36f), 0f)
    }

    @Test fun favoritesRealEntriesUseTheListsOwnRules() {
        val cat = listOf(choice("claude-code", "opus"), choice("codex", "gpt"))
        val list = ModelPickerRules.entries(cat, listOf(FavoriteModel("codex", "gpt")), "", null, locked = false)
        val layout = RailRules.layout(list, headers = true)
        assertEquals(listOf("favorites", "claude-code"), layout.sections.map { it.id })
        assertEquals("favorites", layout.sections[0].harness)
    }
}

class EffortFxTest {
    @Test fun dampingIsSignedBoundedAndStartsNearlyLinear() {
        assertEquals(0f, EffortFx.damp(0f, 14f), 0f)
        assertEquals(-EffortFx.damp(30f, 14f), EffortFx.damp(-30f, 14f), 1e-6f)
        assertTrue(EffortFx.damp(1000f, 14f) <= 14f)
        assertTrue(EffortFx.damp(1000f, 14f) > 13.9f)
        // Close to the end the thumb follows at roughly 0.6:1.
        assertEquals(0.625f, EffortFx.damp(0.5f, 14f) / 0.5f, 0.02f)
    }

    @Test fun dampingResistsMoreTheFurtherYouPull() {
        var lastOver = 0f
        var last = 0f
        var lastSlope = Float.MAX_VALUE
        for (over in listOf(2f, 6f, 12f, 24f, 48f, 96f)) {
            val d = EffortFx.damp(over, 14f)
            assertTrue("monotonic at $over", d > last)
            val slope = (d - last) / (over - lastOver)
            assertTrue("each extra pixel moves the thumb less ($slope vs $lastSlope)", slope < lastSlope)
            lastSlope = slope
            lastOver = over
            last = d
        }
    }

    @Test fun overshootIsZeroBetweenTheStopsAndSignedBeyondThem() {
        assertEquals(0f, EffortFx.overshoot(0.4f, 200f), 0f)
        assertEquals(0f, EffortFx.overshoot(1f, 200f), 0f)
        assertEquals(20f, EffortFx.overshoot(1.1f, 200f), 1e-4f)
        assertEquals(-40f, EffortFx.overshoot(-0.2f, 200f), 1e-4f)
    }

    @Test fun rawFractionIsUnclamped() {
        assertEquals(1.5f, EffortScale.rawFractionAt(200f, 200f, 50f), 1e-5f)
        assertEquals(-0.5f, EffortScale.rawFractionAt(0f, 200f, 50f), 1e-5f)
        assertEquals(1f, EffortScale.fractionAt(200f, 200f, 50f), 0f)
    }

    @Test fun colorsRunCoolToWarmToHot() {
        fun r(c: Int) = (c ushr 16) and 0xFF
        fun b(c: Int) = c and 0xFF
        val low = EffortFx.startColor(0f)
        val mid = EffortFx.startColor(0.5f)
        val high = EffortFx.startColor(1f)
        assertTrue("cool is blue-ish", b(low) > r(low))
        assertTrue("hot is red-ish", r(high) > b(high) + 100)
        assertTrue("warm sits between in red", r(low) < r(mid) && r(mid) <= 0xFF)
        assertNotEquals(low, mid)
        assertNotEquals(mid, high)
        // Endpoints are exact and the ramp is continuous across the midpoint.
        assertEquals(0xFF3FA7D6.toInt(), low)
        assertTrue(kotlin.math.abs(r(EffortFx.endColor(0.499f)) - r(EffortFx.endColor(0.501f))) <= 2)
        // Out-of-range levels clamp.
        assertEquals(EffortFx.endColor(1f), EffortFx.endColor(7f))
        assertEquals(EffortFx.endColor(0f), EffortFx.endColor(-1f))
    }

    @Test fun lifeScalesWithTheLevel() {
        val levels = listOf(0f, 0.25f, 0.5f, 0.75f, 1f)
        assertEquals(levels.sortedBy { EffortFx.shimmerSpeed(it) }, levels)
        assertEquals(levels.sortedBy { EffortFx.shimmerAlpha(it) }, levels)
        assertTrue(EffortFx.shimmerSpeed(1f) > 3 * EffortFx.shimmerSpeed(0f))
        assertEquals(0, EffortFx.sparkleCount(0f))
        assertEquals(0, EffortFx.sparkleCount(0.49f))
        assertTrue(EffortFx.sparkleCount(0.6f) in 1..4)
        assertEquals(EffortFx.MAX_SPARKLES, EffortFx.sparkleCount(1f))
        assertEquals(0f, EffortFx.glow(0.5f), 0f)
        assertEquals(1f, EffortFx.glow(1f), 1e-6f)
        assertTrue(EffortFx.sparkleRate(1f) > EffortFx.sparkleRate(0.5f))
    }

    @Test fun sparklesAreDeterministicAndBounded() {
        for (i in 0 until EffortFx.MAX_SPARKLES) {
            for (t in listOf(0f, 0.37f, 1.9f, 55.5f)) {
                val life = EffortFx.sparkleLife(i, t)
                assertTrue(life in 0f..1f)
                val c = EffortFx.sparkleCycle(i, t)
                assertTrue(EffortFx.sparkleX(i, c) in 0f..1f)
                assertTrue(EffortFx.sparkleY(i, c) in 0.18f..0.82f)
                assertTrue(EffortFx.sparkleSize(i, c) in 0.6f..1f)
                assertEquals(life, EffortFx.sparkleLife(i, t), 0f)
            }
        }
    }

    @Test fun zipStreaksAreStaggeredAndFinish() {
        for (i in 0 until EffortFx.ZIP_STREAKS) {
            assertEquals(0f, EffortFx.zipProgress(i, 0f), 0f)
            assertEquals(1f, EffortFx.zipProgress(i, 1f), 1e-6f)
            assertEquals(0f, EffortFx.zipAlpha(0f), 0f)
            assertEquals(0f, EffortFx.zipAlpha(1f), 0f)
        }
        assertTrue((0 until EffortFx.ZIP_STREAKS).map { EffortFx.zipProgress(it, 0.4f) }.toSet().size > 1)
    }
}

class ArrivalTrackerTest {
    @Test fun endsAreTheFirstAndLastStops() {
        val t = ArrivalTracker()
        assertEquals(EffortEnd.Low, t.endOf(0, 5))
        assertEquals(EffortEnd.High, t.endOf(4, 5))
        assertNull(t.endOf(2, 5))
        assertNull(t.endOf(0, 1))
    }

    @Test fun playsOncePerVisitAndOnlyAfterTheUserMovedThere() {
        val t = ArrivalTracker()
        assertEquals(EffortEnd.High, t.settle(4, 5, moved = true))
        assertNull("not again while it stays", t.settle(4, 5, moved = true))
        assertNull(t.settle(3, 5, moved = true))
        assertEquals("re-armed after leaving", EffortEnd.High, t.settle(4, 5, moved = true))
        assertEquals(EffortEnd.Low, t.settle(0, 5, moved = true))
    }

    @Test fun anExternalChangeOrOpeningOnAnEndPlaysNothing() {
        val t = ArrivalTracker()
        t.seed(4, 5)
        assertNull(t.settle(4, 5, moved = true))
        val u = ArrivalTracker()
        assertNull(u.settle(4, 5, moved = false))
        assertEquals("a later user visit still plays", EffortEnd.High, u.settle(4, 5, moved = true))
    }
}

class LightningTest {
    private fun bolt(seed: Long, w: Float = 344f, h: Float = 300f): BoltBuffer =
        BoltBuffer().also { Lightning.generate(seed, w - 40f, 60f, w, h, it) }

    private fun BoltBuffer.dump() = (0 until count).map { listOf(x1[it], y1[it], x2[it], y2[it], gen[it].toFloat()) }

    @Test fun sameSeedSameStrikeDifferentSeedDifferent() {
        assertEquals(bolt(42).dump(), bolt(42).dump())
        assertNotEquals(bolt(42).dump(), bolt(43).dump())
    }

    @Test fun regeneratingIntoAReusedBufferMatchesAFreshOne() {
        val reused = BoltBuffer()
        Lightning.generate(7, 300f, 60f, 344f, 300f, reused)
        Lightning.generate(99, 300f, 60f, 344f, 300f, reused)
        val fresh = BoltBuffer().also { Lightning.generate(99, 300f, 60f, 344f, 300f, it) }
        assertEquals(fresh.dump(), reused.dump())
    }

    @Test fun segmentCountIsBoundedAndNonEmptyAcrossManySeeds() {
        for (seed in 1L..400L) {
            val b = bolt(seed)
            assertTrue("seed $seed", b.count in 32..Lightning.MAX_SEGMENTS)
        }
    }

    @Test fun aSmallBufferIsFilledNotOverrun() {
        val tiny = BoltBuffer(10)
        Lightning.generate(5, 300f, 60f, 344f, 300f, tiny)
        assertEquals(10, tiny.count)
    }

    @Test fun theTrunkStartsAtTheAnchorAndSegmentsChain() {
        val b = bolt(11)
        // Forks sprout before the trunk's first half is emitted, so find the first trunk segment.
        val first = (0 until b.count).first { b.gen[it].toInt() == 0 }
        assertEquals(304f, b.x1[first], 1e-3f)
        assertEquals(60f, b.y1[first], 1e-3f)
        var forks = 0
        for (i in 0 until b.count) if (b.gen[i].toInt() > 0) forks++
        assertTrue(forks > 0 || b.count >= 64)
        // The trunk is one continuous polyline: each trunk segment starts where the previous ended.
        var prev = first
        for (i in first + 1 until b.count) {
            if (b.gen[i].toInt() != 0) continue
            if (b.x1[i] == b.x2[prev] && b.y1[i] == b.y2[prev]) prev = i else break
        }
        assertTrue(prev > first)
    }

    @Test fun strikesStayNearThePanel() {
        for (seed in 1L..100L) {
            val b = bolt(seed)
            for (i in 0 until b.count) {
                for (v in listOf(b.x1[i], b.x2[i])) assertTrue("x $v seed $seed", v in -250f..600f)
                for (v in listOf(b.y1[i], b.y2[i])) assertTrue("y $v seed $seed", v in -250f..600f)
            }
        }
    }

    @Test fun degeneratePanelsMakeNothing() {
        val b = BoltBuffer()
        Lightning.generate(1, 0f, 0f, 0f, 0f, b)
        assertEquals(0, b.count)
    }

    @Test fun theFlashEnvelopeStrikesHardThenFlickersOut() {
        assertEquals(0f, LightningFx.intensity(-0.1f), 0f)
        assertEquals(0f, LightningFx.intensity(1f), 0f)
        assertTrue(LightningFx.intensity(0.0f) > 0.95f)
        for (i in 0..100) assertTrue(LightningFx.intensity(i / 100f) in 0f..1f)
        // A restrike lifts it again after a dip.
        val dip = LightningFx.intensity(0.12f)
        val restrike = LightningFx.intensity(0.18f)
        assertTrue(restrike > dip)
        assertTrue(LightningFx.intensity(0.9f) < 0.1f)
    }

    @Test fun strikeGapsAreBetweenEightyAndOneEightyAndOutlastTheFlash() {
        assertEquals(0.8f, LightningFx.gapSeconds(0f), 1e-6f)
        assertEquals(1.8f, LightningFx.gapSeconds(1f), 1e-6f)
        assertTrue(LightningFx.LIFE_SECONDS < LightningFx.gapSeconds(0f))
    }
}
