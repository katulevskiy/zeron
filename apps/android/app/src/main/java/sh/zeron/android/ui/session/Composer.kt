package sh.zeron.android.ui.session

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.spring
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
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
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import sh.zeron.android.core.ComposerChip
import sh.zeron.android.core.DeliveryMode
import sh.zeron.android.core.MentionIndex
import sh.zeron.android.core.StagedImage
import sh.zeron.android.design.Anchor
import sh.zeron.android.design.BrandMark
import sh.zeron.android.design.Chip
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.GlyphIcon
import sh.zeron.android.design.LocalColors
import sh.zeron.android.design.LocalOverlays
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.PrIcon
import sh.zeron.android.design.ProjectTile
import sh.zeron.android.design.SvgIcon
import sh.zeron.android.design.ZText
import sh.zeron.android.design.ZType
import sh.zeron.android.design.anchor
import sh.zeron.android.design.glass
import sh.zeron.android.design.horizontalEdgeFade
import sh.zeron.android.design.pressable
import sh.zeron.android.design.rememberAnchor
import uniffi.zeron_core.FileMatch
import uniffi.zeron_core.fileMentionLink

/** Everything the composer holds between recompositions (text, photos, mentions). */
@Stable
class ComposerState(initial: String = "") {
    var value by mutableStateOf(TextFieldValue(initial, TextRange(initial.length)))
    val images = mutableStateListOf<StagedImage>()
    var focused by mutableStateOf(false)
    val mentions = MentionIndex { path, isDir -> fileMentionLink(path, isDir) }

    val text: String get() = value.text
    val hasContent: Boolean get() = value.text.isNotBlank() || images.isNotEmpty()

    fun setText(text: String) {
        value = TextFieldValue(text, TextRange(text.length))
    }

    fun clear() {
        mentions.reset()
        setText("")
        images.clear()
    }

    /** The message as sent: trimmed, `@tokens` encoded as canonical links. */
    fun outgoing(): String = mentions.encode(value.text.trim())

    fun addImages(new: List<StagedImage>) {
        images.addAll(new.take((sh.zeron.android.core.Attachments.MAX_IMAGES - images.size).coerceAtLeast(0)))
    }
}

private enum class Action { Send, Queue, Stop }

/** What happened to a send, so the composer knows whether to clear. */
enum class SendResult {
    /** Taken: the composer clears. */
    Sent,

    /** Refused (e.g. the core couldn't start it): text and photos stay. */
    Kept,

    /** Taken, and the handler already set the composer's next contents. */
    Replaced,
}

/**
 * The composer: one glass surface in two states that morph into each other —
 * resting, a one-line capsule `[+] Message… [↑]`; composing (focused, holding
 * photos, or pinned open on the new-session page), a card with photos,
 * full-width text and a toolbar `[+] [model] [effort] [branch] … [↑]`.
 * The action button is Send, Queue (long-press: steer / stop-and-send) or Stop.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun Composer(
    state: ComposerState,
    placeholder: String,
    running: Boolean,
    canSteer: Boolean,
    chips: List<ComposerChip>,
    focus: FocusRequester,
    modifier: Modifier = Modifier,
    alwaysCard: Boolean = false,
    onChip: (ComposerChip, Anchor) -> Unit = { _, _ -> },
    onAttach: () -> Unit,
    onSend: (text: String, images: List<StagedImage>, mode: DeliveryMode) -> SendResult,
    onStop: () -> Unit = {},
    mentionSearch: (suspend (String) -> List<FileMatch>)? = null,
) {
    val c = LocalColors.current
    val haptics = LocalHapticFeedback.current
    val overlays = LocalOverlays.current
    val card = alwaysCard || state.focused || state.images.isNotEmpty()
    val action = when {
        !running -> Action.Send
        state.hasContent -> Action.Queue
        else -> Action.Stop
    }

    fun send(mode: DeliveryMode) {
        if (!state.hasContent) return
        haptics.performHapticFeedback(HapticFeedbackType.LongPress)
        when (onSend(state.outgoing(), state.images.toList(), mode)) {
            SendResult.Sent -> state.clear()
            // The handler put the next contents in place (a committed queue
            // edit restores the stashed draft) — its @mentions must still resolve.
            SendResult.Replaced, SendResult.Kept -> Unit
        }
    }

    // `@query` → file suggestions (debounced; stale answers dropped).
    val query = remember(state.value) { MentionIndex.activeQuery(state.value.text, state.value.selection.start) }
    var suggestions by remember { mutableStateOf<List<FileMatch>>(emptyList()) }
    LaunchedEffect(query?.second, mentionSearch) {
        val q = query ?: return@LaunchedEffect run { suggestions = emptyList() }
        val search = mentionSearch ?: return@LaunchedEffect
        delay(120)
        suggestions = runCatching { search(q.second) }.getOrDefault(emptyList()).take(6)
    }

    fun insertMention(file: FileMatch) {
        val q = query ?: return
        val token = state.mentions.token(MentionIndex.File(file.path, file.isDir)) + " "
        val t = state.value.text
        val end = state.value.selection.start
        val next = t.substring(0, q.first) + token + t.substring(end)
        state.value = TextFieldValue(next, TextRange(q.first + token.length))
        suggestions = emptyList()
    }

    Column(modifier) {
        AnimatedVisibility(
            suggestions.isNotEmpty() && query != null && state.focused,
            enter = fadeIn() + slideInVertically { it / 4 },
            exit = fadeOut() + slideOutVertically { it / 4 },
        ) {
            MentionSuggestions(suggestions) { insertMention(it) }
        }
        Column(
            Modifier
                .fillMaxWidth()
                .glass(c, RoundedCornerShape(if (card) 26.dp else 25.dp), 10.dp)
                .animateContentSize(spring(dampingRatio = 0.86f, stiffness = 520f)),
        ) {
            if (state.images.isNotEmpty()) Thumbs(state)
            Row(verticalAlignment = Alignment.Bottom) {
                if (!card) {
                    Box(Modifier.padding(start = 8.dp, bottom = 8.dp)) { AttachButton(onAttach) }
                }
                val tokens = state.mentions.liveTokens
                BasicTextField(
                    value = state.value,
                    onValueChange = { state.value = it },
                    modifier = Modifier
                        .weight(1f)
                        .padding(start = if (card) 16.dp else 10.dp, end = if (card) 16.dp else 8.dp)
                        .heightIn(min = if (card) 46.dp else 50.dp)
                        .padding(top = 14.dp, bottom = if (card) 6.dp else 14.dp)
                        .focusRequester(focus)
                        .onFocusChanged { state.focused = it.isFocused }
                        .semantics { contentDescription = "Message" },
                    textStyle = ZType.sans(16.5f, lineHeight = 22f).copy(color = c.text),
                    cursorBrush = SolidColor(c.accent),
                    maxLines = if (card) 8 else 2,
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
                    visualTransformation = remember(tokens.size, c) { MentionHighlight(tokens, c.accent) },
                    decorationBox = { inner ->
                        Box {
                            if (state.value.text.isEmpty()) ZText(placeholder, ZType.sans(16.5f, lineHeight = 22f), c.tertiary, maxLines = 1)
                            inner()
                        }
                    },
                )
                if (!card) {
                    Box(Modifier.padding(end = 8.dp, bottom = 8.dp)) {
                        ActionButton(action, state.hasContent, canSteer, onPrimary = {
                            when (action) {
                                Action.Stop -> onStop()
                                else -> send(DeliveryMode.Queue)
                            }
                        }, onMode = { send(it) }, overlays = overlays)
                    }
                }
            }
            if (card) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .height(50.dp)
                        .padding(horizontal = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    AttachButton(onAttach)
                    Spacer(Modifier.width(8.dp))
                    val chipScroll = rememberScrollState()
                    Row(
                        Modifier
                            .weight(1f)
                            .horizontalEdgeFade(chipScroll)
                            .horizontalScroll(chipScroll),
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        for (chip in chips) ComposerChipView(chip, onChip)
                    }
                    Spacer(Modifier.width(8.dp))
                    ActionButton(action, state.hasContent, canSteer, onPrimary = {
                        when (action) {
                            Action.Stop -> onStop()
                            else -> send(DeliveryMode.Queue)
                        }
                    }, onMode = { send(it) }, overlays = overlays)
                }
            }
        }
    }
}

@Composable
private fun ComposerChipView(chip: ComposerChip, onChip: (ComposerChip, Anchor) -> Unit) {
    val c = LocalColors.current
    val anchor = rememberAnchor()
    val tint = when (chip.kind) {
        ComposerChip.Kind.PrOpen -> c.success
        ComposerChip.Kind.PrMerged -> c.accent
        ComposerChip.Kind.PrClosed -> c.danger
        ComposerChip.Kind.Context -> if (chip.warn) c.warning else null
        else -> null
    }
    val ink = tint?.copy(alpha = 0.85f) ?: c.text
    Chip(
        chip.title,
        Modifier.anchor(anchor),
        tint = tint,
        mono = chip.kind == ComposerChip.Kind.PrOpen || chip.kind == ComposerChip.Kind.PrMerged || chip.kind == ComposerChip.Kind.PrClosed,
        leading = {
            when (chip.kind) {
                ComposerChip.Kind.Model -> BrandMark(chip.harness, 14.dp)
                ComposerChip.Kind.Effort -> GlyphIcon(Glyph.Gauge, ink, size = 14.dp)
                ComposerChip.Kind.Branch -> SvgIcon("tool-git-branch", 13.dp, ink)
                ComposerChip.Kind.PrOpen, ComposerChip.Kind.PrMerged, ComposerChip.Kind.PrClosed -> PrIcon(ink, 12.dp)
                ComposerChip.Kind.Context -> GlyphIcon(if (chip.warn) Glyph.Warning else Glyph.HalfCircle, ink, size = 13.dp)
                ComposerChip.Kind.Project -> if (chip.colorIndex != null) ProjectTile(chip.title, chip.colorIndex, size = 14.dp) else GlyphIcon(Glyph.Tray, ink, size = 14.dp)
                ComposerChip.Kind.Host -> GlyphIcon(Glyph.Desktop, ink, size = 14.dp)
                ComposerChip.Kind.Plain -> Unit
            }
        },
    ) { onChip(chip, anchor) }
}

@Composable
private fun AttachButton(onClick: () -> Unit) {
    val c = LocalColors.current
    Box(
        Modifier
            .size(34.dp)
            .background(c.controlFill, CircleShape)
            .pressable(shape = CircleShape, label = "Attach photos", onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { GlyphIcon(Glyph.Plus, c.text, size = 18.dp, weight = 2f) }
}

/** Send ⇄ Queue ⇄ Stop, morphing in place. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ActionButton(
    action: Action,
    hasContent: Boolean,
    canSteer: Boolean,
    onPrimary: () -> Unit,
    onMode: (DeliveryMode) -> Unit,
    overlays: sh.zeron.android.design.Overlays,
) {
    val c = LocalColors.current
    val anchor = rememberAnchor()
    val enabled = action == Action.Stop || hasContent
    val bg = when (action) {
        Action.Stop -> c.text
        else -> if (enabled) c.accent else c.controlFill
    }
    val label = when (action) {
        Action.Send -> "Send message"
        Action.Queue -> "Queue message"
        Action.Stop -> "Stop response"
    }
    Box(
        Modifier
            .anchor(anchor)
            .size(34.dp)
            .clip(CircleShape)
            .background(bg)
            .combinedClickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
                enabled = enabled,
                role = Role.Button,
                onClickLabel = label,
                onLongClick = if (action == Action.Queue) {
                    {
                        overlays.menu(anchor, "While the agent works", buildList {
                            add(MenuEntry.Item("Queue for next turn", glyph = Glyph.Queue) { onMode(DeliveryMode.Queue) })
                            if (canSteer) add(MenuEntry.Item("Steer now", subtitle = "Deliver into the running turn", glyph = Glyph.Steer) { onMode(DeliveryMode.Steer) })
                            add(MenuEntry.Item("Stop and send", glyph = Glyph.StopCircle, destructive = true) { onMode(DeliveryMode.Interrupt) })
                        })
                    }
                } else null,
                onClick = onPrimary,
            )
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        AnimatedContent(action, transitionSpec = { (scaleIn(initialScale = 0.6f) + fadeIn()) togetherWith (scaleOut(targetScale = 0.6f) + fadeOut()) }, label = "action") { a ->
            when (a) {
                Action.Stop -> GlyphIcon(Glyph.Stop, c.background, size = 20.dp)
                else -> GlyphIcon(Glyph.ArrowUp, if (enabled) Color.White else c.tertiary, size = 18.dp, weight = 2.4f)
            }
        }
    }
}

@Composable
private fun Thumbs(state: ComposerState) {
    val c = LocalColors.current
    Row(
        Modifier
            .fillMaxWidth()
            .horizontalScroll(rememberScrollState())
            .padding(start = 12.dp, end = 12.dp, top = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        for (img in state.images) {
            Box(Modifier.size(56.dp)) {
                Image(
                    img.thumbnail.asImageBitmap(), contentDescription = "Attached photo",
                    contentScale = ContentScale.Crop,
                    modifier = Modifier
                        .size(56.dp)
                        .clip(RoundedCornerShape(14.dp))
                        .border(0.75.dp, c.hairline, RoundedCornerShape(14.dp)),
                )
                Box(
                    Modifier
                        .align(Alignment.TopEnd)
                        .padding(4.dp)
                        .size(18.dp)
                        .background(Color.Black.copy(alpha = 0.55f), CircleShape)
                        .pressable(shape = CircleShape, label = "Remove photo") { state.images.remove(img) },
                    contentAlignment = Alignment.Center,
                ) { GlyphIcon(Glyph.Close, Color.White, size = 10.dp, weight = 3f) }
            }
        }
    }
}

@Composable
private fun MentionSuggestions(files: List<FileMatch>, onPick: (FileMatch) -> Unit) {
    val c = LocalColors.current
    Column(
        Modifier
            .padding(bottom = 8.dp)
            .fillMaxWidth()
            .glass(c, RoundedCornerShape(20.dp), 12.dp)
            .padding(vertical = 4.dp),
    ) {
        for (f in files) {
            Row(
                Modifier
                    .fillMaxWidth()
                    .defaultMinSize(minHeight = 42.dp)
                    .pressable(highlight = c.controlFill, dim = 1f, shape = RoundedCornerShape(12.dp)) { onPick(f) }
                    .padding(horizontal = 14.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                GlyphIcon(if (f.isDir) Glyph.Folder else Glyph.File, c.secondary, size = 16.dp)
                Spacer(Modifier.width(10.dp))
                val name = f.path.trimEnd('/').substringAfterLast('/')
                ZText(name, ZType.sans(15f, FontWeight.Medium), c.text, maxLines = 1)
                Spacer(Modifier.width(8.dp))
                ZText(f.path.trimEnd('/').substringBeforeLast('/', ""), ZType.mono(12f), c.tertiary, Modifier.weight(1f), maxLines = 1)
            }
        }
    }
}

/** Accent the live `@tokens`; offsets are untouched (identity mapping). */
private class MentionHighlight(private val tokens: Set<String>, private val accent: Color) : VisualTransformation {
    override fun filter(text: AnnotatedString): TransformedText {
        if (tokens.isEmpty()) return TransformedText(text, OffsetMapping.Identity)
        val s = text.text
        val styled = buildAnnotatedString {
            append(text)
            for (t in tokens) {
                var i = s.indexOf(t)
                while (i >= 0) {
                    addStyle(SpanStyle(color = accent, fontWeight = FontWeight.Medium), i, i + t.length)
                    i = s.indexOf(t, i + t.length)
                }
            }
        }
        return TransformedText(styled, OffsetMapping.Identity)
    }

    override fun equals(other: Any?) = other is MentionHighlight && other.tokens == tokens && other.accent == accent
    override fun hashCode() = tokens.hashCode() * 31 + accent.hashCode()
}
