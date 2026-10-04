package sh.zeron.android.voice.screen

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class ScreenPolicyTest {
    private fun screen(created: Long = 100) = ScreenSnapshot("fixture", 1, 1080, 2400, 0, "Settings",
        listOf(ScreenElement("1", "Display", "Button", listOf(0, 50, 100, 100), listOf("CLICK")),
            ScreenElement("2", "Search", "EditText", listOf(0, 100, 200, 180), listOf("TYPE"))), created)
    private fun answer(choice: String, options: List<String>) = JSONObject().put("type", "choice").put("choice", choice).put("confidence", .99)
        .put("probabilities", JSONObject().apply { options.forEach { put(it, if (it == choice) 1.0 else 0.0) } })
    private fun response(request: JSONObject, operation: String): JSONObject {
        val questions = request.getJSONObject("questions")
        return JSONObject().put("answers", JSONObject().apply {
            put("operation", answer(operation, questions.getJSONObject("operation").getJSONObject("criteria").keys().asSequence().toList()))
            if (questions.has(operation.lowercase()+"_target")) {
                val targets = questions.getJSONObject(operation.lowercase()+"_target").getJSONObject("criteria").keys().asSequence().toList()
                put(operation.lowercase()+"_target", answer(targets.first(), targets))
            }
        })
    }
    @Test fun operationAndTargetsShareOneRequestAndTypingNeedsExactText() {
        val request = ScreenPolicy.request(screen(), "Open Display", null, emptyList())
        assertTrue(request.getJSONObject("questions").has("click_target"))
        assertFalse(request.getJSONObject("questions").has("type_target"))
        val decision = ScreenPolicy.decode(request, response(request, "CLICK"), 234)
        assertEquals("1", decision.target); assertEquals(234, decision.latencyMs)
        val typed = ScreenPolicy.request(screen(), "Search for display", "display", emptyList())
        assertTrue(typed.getJSONObject("questions").has("type_target"))
    }
    @Test fun malformedOrUncertainDecisionsNeverBecomeInput() {
        val request = ScreenPolicy.request(screen(), "Open Display", null, emptyList())
        val good = response(request, "CLICK")
        fun rejected(transform: (JSONObject) -> Unit) {
            val bad = JSONObject(good.toString()); transform(bad)
            try { ScreenPolicy.decode(request, bad, 1); fail("Invalid response accepted") } catch (_: Exception) { }
        }
        rejected { it.getJSONObject("answers").getJSONObject("operation").put("choice", "EXECUTE_SHELL") }
        rejected { it.getJSONObject("answers").getJSONObject("operation").put("confidence", .1) }
        rejected { it.getJSONObject("answers").getJSONObject("click_target").put("choice", "999") }
        rejected { it.getJSONObject("answers").getJSONObject("click_target").getJSONObject("probabilities").put("unobserved", 0) }
    }
    @Test fun changedExpiredRotatedScreensAndInvalidCoordinatesAreRejected() {
        ScreenPolicy.requireFresh(screen(), screen(), 120)
        for (current in listOf(screen().copy(text="Other screen"), screen().copy(rotation=1), screen().copy(windowId=2))) {
            try { ScreenPolicy.requireFresh(screen(), current, 120); fail("Stale decision accepted") } catch (_: IllegalArgumentException) { }
        }
        try { ScreenPolicy.requireFresh(screen(), screen(), 5200); fail() } catch (_: IllegalArgumentException) { }
        for ((x,y) in listOf(-1.0 to 50.0, 1080.0 to 50.0, 1.0 to Double.NaN, Double.POSITIVE_INFINITY to 1.0)) {
            try { ScreenPolicy.requirePoint(x,y,screen()); fail() } catch (_: IllegalArgumentException) { }
        }
        ScreenPolicy.requirePoint(400.0,800.0,screen())
    }
    @Test fun unusedSpeculativeHeadCannotTriggerTyping() {
        val request = ScreenPolicy.request(screen(), "Open Display", "secret draft", emptyList())
        val answer = response(request,"CLICK").apply { getJSONObject("answers").put("type_target", JSONObject().put("choice", "999")) }
        assertEquals("CLICK",ScreenPolicy.decode(request, answer, 1).operation)
    }
}
