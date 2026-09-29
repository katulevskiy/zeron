package sh.zeron.android.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.Connectivity
import uniffi.zeron_core.ConnectivityState
import uniffi.zeron_core.FrontPage
import uniffi.zeron_core.SectionView
import uniffi.zeron_core.SendState

class FormattersTest {
    @Test fun elapsed() {
        assertEquals("42s", Formatters.elapsed(42))
        assertEquals("3m 7s", Formatters.elapsed(187))
        assertEquals("1h 4m", Formatters.elapsed(3840))
    }

    @Test fun liveSummary() {
        assertEquals("", Formatters.liveSummary(0, 0))
        assertEquals("2 working · 1 needs you", Formatters.liveSummary(2, 1))
        assertEquals("3 need you", Formatters.liveSummary(0, 3))
    }

    @Test fun paths() {
        assertEquals("/home/zeron", Formatters.parentPath("/home/zeron/projects"))
        assertEquals("/", Formatters.parentPath("/home"))
        assertNull(Formatters.parentPath("/"))
        assertEquals("/a/b", Formatters.joinPath("/a/", "b"))
        assertEquals("projects", Formatters.lastComponent("/home/zeron/projects/"))
    }

    @Test fun repoNames() {
        assertEquals("widgets", Formatters.repoName("https://github.com/acme/widgets.git"))
        assertEquals("widgets", Formatters.repoName("git@github.com:acme/widgets.git"))
        assertEquals("dot.files", Formatters.repoName("https://example.com/me/dot.files/"))
        assertNull(Formatters.repoName("widgets"))
        assertNull(Formatters.repoName("https://example.com/a b"))
    }

    @Test fun shellQuote() {
        assertEquals("'it'\\''s'", Formatters.shellQuote("it's"))
    }

    @Test fun contextChip() {
        assertNull(Formatters.contextChip(40, 100))
        assertEquals("50% context", Formatters.contextChip(50, 100))
        assertNull(Formatters.contextChip(null, 100))
        assertNull(Formatters.contextChip(10, 0))
    }
}

class MentionsTest {
    private val index = MentionIndex { path, dir -> "[${path.substringAfterLast('/')}](zeron-file:$path${if (dir) "/" else ""})" }

    @Test fun activeQuery() {
        assertEquals(0 to "src", MentionIndex.activeQuery("@src", 4))
        assertEquals(4 to "ma", MentionIndex.activeQuery("fix @ma", 7))
        assertNull(MentionIndex.activeQuery("mail@host", 9))
        assertNull(MentionIndex.activeQuery("@src done", 9))
    }

    @Test fun tokensDisambiguateAndEncodeLongestFirst() {
        val a = index.token(MentionIndex.File("src/main.rs", false))
        val b = index.token(MentionIndex.File("bin/main.rs", false))
        assertEquals("@main.rs", a)
        assertEquals("@bin/main.rs", b)
        val out = index.encode("look at @bin/main.rs and @main.rs")
        assertEquals("look at [main.rs](zeron-file:bin/main.rs) and [main.rs](zeron-file:src/main.rs)", out)
    }

    @Test fun queueTextKeepsHiddenContext() {
        assertEquals("new\n\n[attachments]", QueueText.replacingVisible("old\n\n[attachments]", "old", "new"))
        assertEquals("blank edits stay blank (the host removes the row)", "  ", QueueText.replacingVisible("old ctx", "old", "  "))
        assertEquals("edit\n\n[img]", QueueText.replacingVisible("[img]", "Attachment", "edit"))
    }
}

class SessionListsTest {
    private fun front(pinned: List<String>, sections: List<Pair<String, List<String>>>, recent: List<String>, collapsed: Set<String> = emptySet()) = FrontPage(
        pinned.map { Fixtures.row(it, pinned = true) },
        sections.map { (id, rows) -> SectionView(id, id.uppercase(), id in collapsed, rows.map { Fixtures.row(it) }) },
        recent.map { Fixtures.row(it) },
    )

    @Test fun recentAloneHasNoHeader() {
        val items = SessionLists.frontPage(front(emptyList(), emptyList(), listOf("a", "b")), emptySet())
        assertEquals(listOf("s:a", "s:b"), items.map { it.key })
    }

    @Test fun groupsFoldAndDedupe() {
        val f = front(listOf("p"), listOf("work" to listOf("w1", "p")), listOf("r", "w1"), collapsed = setOf("work"))
        val items = SessionLists.frontPage(f, setOf(SessionLists.RECENT))
        assertEquals(listOf("h:pinned", "s:p", "h:work", "h:recent"), items.map { it.key })
        val work = items[2] as ListItem.Header
        assertTrue(work.collapsed)
        assertEquals(1, work.count) // "p" already shown under Pinned
        assertEquals(1, (items[3] as ListItem.Header).count) // "w1" belongs to its section
        assertEquals(items.size, items.map { it.key }.toSet().size)
    }

    @Test fun cornerPrecedence() {
        assertEquals(Corner.SendFailed, SessionLists.corner(Fixtures.row("a", ChatIndicator.WORKING, sendState = SendState.FAILED)))
        assertEquals(Corner.Working, SessionLists.corner(Fixtures.row("a", ChatIndicator.WORKING)))
        assertEquals(Corner.Input, SessionLists.corner(Fixtures.row("a", ChatIndicator.AWAITING_INPUT)))
        assertEquals(Corner.Done, SessionLists.corner(Fixtures.row("a", ChatIndicator.COMPLETED, unseen = true)))
        assertNull(SessionLists.corner(Fixtures.row("a", ChatIndicator.COMPLETED)))
        assertNull(SessionLists.corner(Fixtures.row("a")))
    }

    @Test fun liveMarksAndCounts() {
        val f = FrontPage(emptyList(), emptyList(), listOf(Fixtures.row("a", ChatIndicator.AWAITING_INPUT), Fixtures.row("b", ChatIndicator.WORKING)))
        assertEquals(LiveMark.Working, SessionLists.live(f.recent))
        assertEquals(1 to 1, SessionLists.liveCounts(f))
    }

    @Test fun pinMoves() {
        val order = listOf("a", "b", "c")
        assertEquals("b" to "c", SessionLists.pinMove(order, "a", 1)?.let { it.first to it.second })
        assertEquals(null to "a", SessionLists.pinMove(order, "b", -1))
        assertNull(SessionLists.pinMove(order, "a", -1))
        assertNull(SessionLists.pinMove(order, "c", 1))
    }
}

class NotifierTest {
    @Test fun transitions() {
        assertEquals(Notifier.Kind.Done, Notifier.transition(ChatIndicator.WORKING, ChatIndicator.IDLE))
        assertEquals(Notifier.Kind.Done, Notifier.transition(ChatIndicator.WORKING, ChatIndicator.COMPLETED))
        assertEquals(Notifier.Kind.Input, Notifier.transition(ChatIndicator.WORKING, ChatIndicator.AWAITING_INPUT))
        assertEquals(Notifier.Kind.Failed, Notifier.transition(ChatIndicator.IDLE, ChatIndicator.ERRORED))
        assertNull(Notifier.transition(ChatIndicator.IDLE, ChatIndicator.WORKING))
        assertNull(Notifier.transition(ChatIndicator.IDLE, ChatIndicator.IDLE))
    }
}

class SessionChromeTest {
    private val labels = object : SessionChrome.Labels {
        override fun reasoning(level: String) = level.uppercase()
    }

    @Test fun bannerPrecedence() {
        val row = Fixtures.row("c1")
        val offline = Connectivity(ConnectivityState.OFFLINE, null, null, emptyList())
        assertEquals(Banner.Failed("boom"), SessionChrome.from(Fixtures.composer(sendState = SendState.FAILED), row, offline, "boom", 0, labels).banner)
        assertEquals(Banner.NotDelivered, SessionChrome.from(Fixtures.composer(sendState = SendState.FAILED), row, offline, null, 0, labels).banner)
        assertEquals(Banner.Offline, SessionChrome.from(Fixtures.composer(), row, offline, null, 0, labels).banner)
        assertEquals(Banner.Reconnecting(4), SessionChrome.from(Fixtures.composer(roomConnected = false, retryAtMs = 4_500), row, null, null, 0, labels).banner)
        assertNull(SessionChrome.from(Fixtures.composer(), row, null, null, 0, labels).banner)
    }

    @Test fun titlesAndChips() {
        val chrome = SessionChrome.from(Fixtures.composer(running = true), Fixtures.row("c1").copy(modelLabel = "Opus", reasoning = "high"), null, null, 0, labels)
        assertEquals("zeron @ Mac", chrome.subtitle)
        assertTrue(chrome.running)
        assertEquals(listOf("model", "effort", "branch"), chrome.chips.map { it.id })
        assertEquals("HIGH", chrome.chips[1].title)
        assertEquals("Message Claude Code", chrome.placeholder)
        assertFalse(chrome.chips.any { it.kind == ComposerChip.Kind.Context })
    }
}

class AgentsTest {
    @Test fun harnessCatalog() {
        val json = org.json.JSONArray(
            """[{"id":"claude-code","name":"Claude Code","installed":true,"canInstall":true,"enabled":true},
               {"id":"grok","name":"Grok","installed":false,"canInstall":true},
               {"id":"mock","name":"Mock"},
               {"id":"old","name":"Old engine"}]""",
        )
        val list = Agents.harnesses(json)
        assertEquals(listOf("claude-code", "grok", "old"), list.map { it.id })
        assertFalse(list[1].installed)
        assertNull(list[1].enabled)
        assertTrue("engines predating the field read as installed", list[2].installed)
        assertFalse(list[2].canInstall)
    }

    @Test fun accountsAndLogins() {
        val snap = Agents.accounts(org.json.JSONObject("""{"accounts":[{"id":"a1","harness":"codex","email":"me@x.dev","planLabel":"Pro","active":true}],"warnings":[{"harness":"claude-code","message":"Keychain denied"}]}"""))
        assertEquals("me@x.dev", snap.forHarness("codex").single().title)
        assertEquals("Keychain denied", snap.warnings["claude-code"])
        assertEquals(Agents.LoginMode.PasteCode, Agents.loginStart(org.json.JSONObject("""{"loginId":"l1","url":"https://x","mode":"paste-code"}"""))?.mode)
        assertNull(Agents.loginStart(org.json.JSONObject("{}")))
        assertEquals(Agents.LoginPoll.Done, Agents.loginPoll(org.json.JSONObject("""{"status":"done"}""")))
        assertEquals(Agents.LoginPoll.Failed("nope"), Agents.loginPoll(org.json.JSONObject("""{"status":"error","message":"nope"}""")))
        assertEquals(Agents.LoginPoll.Pending("https://late"), Agents.loginPoll(org.json.JSONObject("""{"status":"pending","url":"https://late"}""")))
        assertEquals("2.1.0", Agents.versions(org.json.JSONArray("""[{"harness":"codex","installedVersion":"2.1.0","phase":"current"}]"""))["codex"]?.installed)
    }

    @Test fun relayTimeouts() {
        assertTrue(Agents.isTimeout("InstallHarness on dev-1 timed out"))
        assertFalse(Agents.isTimeout("dev-1: connection closed"))
    }
}

class DraftTest {
    @Test fun newSessionDraftRoundTrips() {
        val d = NewSessionDraft(projectId = "p1", branch = "main", worktree = true, harness = "codex", model = "gpt-5", effort = "high")
        assertEquals(d, NewSessionDraft.fromJson(d.toJson()))
        assertEquals(NewSessionDraft(), NewSessionDraft.fromJson("not json"))
        assertEquals(NewSessionDraft(), NewSessionDraft.fromJson(null))
    }
}
