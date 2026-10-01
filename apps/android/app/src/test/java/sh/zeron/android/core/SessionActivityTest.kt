package sh.zeron.android.core

import org.junit.Assert.*
import org.junit.Test
import uniffi.zeron_core.ChatIndicator

class SessionActivityTest {
    @Test fun mainRunningWinsEvenWithChildrenAndCallbacks() {
        assertEquals(SessionActivity.Shape.MainRunning, SessionActivity.shape(ChatIndicator.WORKING, 4u, 2u))
        assertEquals(SessionActivity.Shape.MainRunning, SessionActivity.shape(ChatIndicator.WORKING, 0u, 0u))
    }

    @Test fun idleAndUnseenCompletedParentsShowYellowUntilTheLastChildSettles() {
        for (status in listOf(ChatIndicator.IDLE, ChatIndicator.COMPLETED)) {
            assertEquals(SessionActivity.Shape.SubagentsRunning, SessionActivity.shape(status, 1u, 1u))
            assertNull(SessionActivity.shape(status, 0u, 0u))
        }
    }

    @Test fun blueRequiresAnExplicitCallbackAndNoRunningChildren() {
        assertEquals(SessionActivity.Shape.CallbackWaiting, SessionActivity.shape(ChatIndicator.IDLE, 0u, 1u))
        assertEquals(SessionActivity.Shape.SubagentsRunning, SessionActivity.shape(ChatIndicator.IDLE, 2u, 1u))
        assertNull(SessionActivity.shape(ChatIndicator.IDLE, 0u, 0u))
    }

    @Test fun inputAndFailureKeepTheirAttentionLabelsWhileChildrenCanStillBeWorking() {
        for (status in listOf(ChatIndicator.AWAITING_INPUT, ChatIndicator.ERRORED)) {
            assertNull(SessionActivity.shape(status, 3u, 1u))
            assertTrue(SessionActivity.isWorking(status, 3u, 0u))
            assertFalse(SessionActivity.isWorking(status, 0u, 0u))
        }
    }

    @Test fun workingIncludesMainChildrenAndConfirmedCallbacksButNotAnOrdinaryIdleChat() {
        assertTrue(SessionActivity.isWorking(ChatIndicator.WORKING, 0u, 0u))
        assertTrue(SessionActivity.isWorking(ChatIndicator.IDLE, 1u, 0u))
        assertTrue(SessionActivity.isWorking(ChatIndicator.COMPLETED, 0u, 1u))
        assertFalse(SessionActivity.isWorking(ChatIndicator.IDLE, 0u, 0u))
    }

    @Test fun badgeHasNoTextForZeroOrOverflowIcon() {
        assertNull(SessionActivity.badgeLabel(0u))
        assertEquals("1", SessionActivity.badgeLabel(1u))
        assertEquals("9", SessionActivity.badgeLabel(9u))
        assertEquals("10", SessionActivity.badgeLabel(10u))
        assertEquals("99", SessionActivity.badgeLabel(99u))
        assertNull(SessionActivity.badgeLabel(100u))
        assertNull(SessionActivity.badgeLabel(UInt.MAX_VALUE))
    }
}
