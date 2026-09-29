package sh.zeron.android.ui.session

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import sh.zeron.android.core.Banner
import sh.zeron.android.core.QuestionModel
import sh.zeron.android.core.QueueAction
import sh.zeron.android.core.QueuedModel
import sh.zeron.android.design.ButtonStyle
import sh.zeron.android.design.CapsuleButton
import sh.zeron.android.design.FadingText
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.GlyphIcon
import sh.zeron.android.design.GlyphKind
import sh.zeron.android.design.LocalColors
import sh.zeron.android.design.LocalOverlays
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.StatusGlyph
import sh.zeron.android.design.ZText
import sh.zeron.android.design.ZTextField
import sh.zeron.android.design.ZType
import sh.zeron.android.design.anchor
import sh.zeron.android.design.glass
import sh.zeron.android.design.pressable
import sh.zeron.android.design.rememberAnchor

/**
 * Replaces the composer while the agent asks: one card per question, large
 * option rows, "Other…" free text, and single-select auto-advance (220 ms) so
 * a three-question form is three taps. Answers go once per question set.
 */
@Composable
fun QuestionPanel(requestId: String, questions: List<QuestionModel>, onSubmit: (List<Pair<String, List<String>>>) -> Unit) {
    val c = LocalColors.current
    val haptics = LocalHapticFeedback.current
    var page by remember(requestId) { mutableIntStateOf(0) }
    val picks = remember(requestId) { mutableStateMapOf<String, Set<String>>() }
    val custom = remember(requestId) { mutableStateMapOf<String, String>() }
    var submitted by remember(requestId) { mutableStateOf(false) }
    var advanceAt by remember(requestId) { mutableStateOf<Long?>(null) }
    val q = questions.getOrNull(page) ?: return

    fun answered(q: QuestionModel) = !picks[q.id].isNullOrEmpty() || !custom[q.id].isNullOrBlank()

    fun advance() {
        val cur = questions.getOrNull(page) ?: return
        if (submitted || !answered(cur)) return
        if (page < questions.size - 1) {
            page++
            return
        }
        submitted = true
        haptics.performHapticFeedback(HapticFeedbackType.LongPress)
        onSubmit(questions.map { qq ->
            val labels = qq.options.filter { picks[qq.id]?.contains(it) == true }.toMutableList()
            custom[qq.id]?.trim()?.takeIf { it.isNotEmpty() }?.let { labels.add(it) }
            qq.id to labels
        })
    }

    LaunchedEffect(advanceAt) {
        if (advanceAt == null) return@LaunchedEffect
        delay(220)
        advanceAt = null
        advance()
    }

    Column(
        Modifier
            .fillMaxWidth()
            .glass(c, RoundedCornerShape(26.dp), 10.dp)
            .animateContentSize()
            .padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 12.dp),
    ) {
        val header = q.header.ifEmpty { "Question" }
        ZText(if (questions.size > 1) "$header · ${page + 1} of ${questions.size}" else header, ZType.sans(12.5f, FontWeight.Medium), c.secondary)
        Spacer(Modifier.height(10.dp))
        AnimatedContent(page, transitionSpec = { fadeIn() togetherWith fadeOut() }, label = "question") { p ->
            val qq = questions.getOrNull(p) ?: return@AnimatedContent
            Column {
                ZText(qq.text, ZType.sans(17f, FontWeight.SemiBold, lineHeight = 23f), c.text)
                Spacer(Modifier.height(14.dp))
                Column(
                    Modifier
                        .heightIn(max = 300.dp)
                        .verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    qq.options.forEachIndexed { i, option ->
                        val chosen = picks[qq.id]?.contains(option) == true
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .defaultMinSize(minHeight = 46.dp)
                                .background(if (chosen) c.accent.copy(alpha = 0.12f) else c.chip.copy(alpha = 0.55f), RoundedCornerShape(14.dp))
                                .pressable(shape = RoundedCornerShape(14.dp), label = option) {
                                    haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                                    val set = picks[qq.id] ?: emptySet()
                                    picks[qq.id] = if (qq.multiSelect) (if (option in set) set - option else set + option) else setOf(option)
                                    if (!qq.multiSelect) advanceAt = System.nanoTime()
                                }
                                .padding(horizontal = 12.dp, vertical = 11.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            OptionMark(i + 1, chosen, qq.multiSelect)
                            Spacer(Modifier.width(10.dp))
                            ZText(option, ZType.sans(16f, FontWeight.Medium), if (chosen) c.accent else c.text)
                        }
                    }
                }
            }
        }
        Spacer(Modifier.height(10.dp))
        ZTextField(custom[q.id] ?: "", { custom[q.id] = it }, "Other…", onSubmit = { advance() })
        Spacer(Modifier.height(12.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (page > 0) CapsuleButton("Back", style = ButtonStyle.Ghost, compact = true) { page-- }
            Spacer(Modifier.weight(1f))
            CapsuleButton(if (page == questions.size - 1) "Submit" else "Next", compact = true, enabled = answered(q) && !submitted) { advance() }
        }
    }
}

@Composable
private fun OptionMark(n: Int, chosen: Boolean, multi: Boolean) {
    val c = LocalColors.current
    Box(
        Modifier
            .size(22.dp)
            .background(if (chosen) c.accent else c.controlFill, if (multi) RoundedCornerShape(6.dp) else CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        if (chosen) GlyphIcon(Glyph.Check, androidx.compose.ui.graphics.Color.White, size = 15.dp, weight = 2.4f)
        else ZText("$n", ZType.mono(11.5f, FontWeight.Medium), c.secondary)
    }
}

/** Messages waiting for the turn to end, stacked above the composer (first three). */
@Composable
fun QueuePanel(items: List<QueuedModel>, onAction: (String, QueueAction) -> Unit) {
    val c = LocalColors.current
    val overlays = LocalOverlays.current
    Column(
        Modifier
            .padding(horizontal = 10.dp)
            .fillMaxWidth()
            .glass(c, RoundedCornerShape(20.dp), 8.dp)
            .animateContentSize()
            .padding(vertical = 4.dp),
    ) {
        items.take(3).forEachIndexed { index, item ->
            val anchor = rememberAnchor()
            Row(
                Modifier
                    .fillMaxWidth()
                    .height(42.dp)
                    .padding(start = 14.dp, end = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                GlyphIcon(Glyph.Clock, c.tertiary, size = 14.dp)
                Spacer(Modifier.width(9.dp))
                FadingText(item.gate?.let { "$it · ${item.text}" } ?: item.text, ZType.sans(15f), c.text, Modifier.weight(1f))
                Box(
                    Modifier
                        .size(36.dp)
                        .pressable(shape = CircleShape, label = "Send now") { onAction(item.id, QueueAction.SendNow) },
                    contentAlignment = Alignment.Center,
                ) {
                    Box(Modifier.size(22.dp).background(c.text, CircleShape), contentAlignment = Alignment.Center) {
                        GlyphIcon(Glyph.ArrowUp, c.background, size = 13.dp, weight = 2.6f)
                    }
                }
                Box(
                    Modifier
                        .anchor(anchor)
                        .size(36.dp)
                        .pressable(shape = CircleShape, label = "More queue actions") {
                            overlays.menu(anchor, null, listOf(
                                MenuEntry.Item("Edit", glyph = Glyph.Pencil) { onAction(item.id, QueueAction.Edit) },
                                MenuEntry.Item("Move Up", glyph = Glyph.ArrowUp, enabled = index > 0) { onAction(item.id, QueueAction.MoveUp) },
                                MenuEntry.Item("Move Down", glyph = Glyph.ArrowDown, enabled = index < items.size - 1) { onAction(item.id, QueueAction.MoveDown) },
                                MenuEntry.Item("Remove", glyph = Glyph.Trash, destructive = true) { onAction(item.id, QueueAction.Remove) },
                            ))
                        },
                    contentAlignment = Alignment.Center,
                ) { GlyphIcon(Glyph.Ellipsis, c.secondary, size = 18.dp) }
            }
        }
        if (items.size > 3) {
            ZText("+${items.size - 3} more queued", ZType.sans(12.5f, FontWeight.Medium), c.tertiary, Modifier.fillMaxWidth().padding(bottom = 6.dp), align = androidx.compose.ui.text.style.TextAlign.Center)
        }
    }
}

/** Small glass capsule above the composer: offline, reconnecting, retry, editing. */
@Composable
fun StatusPill(banner: Banner, onTap: () -> Unit) {
    val c = LocalColors.current
    val (glyph, text) = when (banner) {
        Banner.Offline -> GlyphKind.Dot(c.statusIdle) to "Offline — sends are saved"
        is Banner.Reconnecting -> GlyphKind.Dot(c.statusIdle) to "Reconnecting in ${banner.seconds}s"
        Banner.NotDelivered -> GlyphKind.Dot(c.danger) to "Not delivered · Tap to retry"
        is Banner.Failed -> GlyphKind.Dot(c.danger) to banner.message
        Banner.Editing -> GlyphKind.Dot(c.statusInput) to "Editing queued message · Tap to cancel"
    }
    Row(
        Modifier
            .height(30.dp)
            .glass(c, CircleShape, 6.dp)
            .pressable(shape = CircleShape, onClick = onTap)
            .padding(start = 11.dp, end = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        StatusGlyph(glyph)
        Spacer(Modifier.width(8.dp))
        ZText(text, ZType.sans(13f, FontWeight.Medium), c.secondary, maxLines = 1)
    }
}
