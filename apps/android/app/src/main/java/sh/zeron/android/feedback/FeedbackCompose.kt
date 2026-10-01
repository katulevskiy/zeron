package sh.zeron.android.feedback

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Indication
import androidx.compose.foundation.IndicationNodeFactory
import androidx.compose.foundation.LocalIndication
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.InteractionSource
import androidx.compose.foundation.interaction.PressInteraction
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.node.CompositionLocalConsumerModifierNode
import androidx.compose.ui.node.DelegatableNode
import androidx.compose.ui.node.DelegatingNode
import androidx.compose.ui.node.currentValueOf
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.Role
import kotlinx.coroutines.launch
import androidx.compose.foundation.clickable as foundationClickable

/**
 * Installs the engine for a subtree: [LocalFeedback], the window view for
 * OEM-tuned haptics, and the press-answering indication that gives every
 * pressable control a default tap (see [ClaimTracker]).
 */
@Composable
fun ProvideFeedback(feedback: AndroidFeedback, content: @Composable () -> Unit) {
    val view = LocalView.current
    DisposableEffect(feedback, view) {
        feedback.attachView(view)
        onDispose { feedback.attachView(null) }
    }
    val base = LocalIndication.current
    val indication = remember(base) { if (base is IndicationNodeFactory) FeedbackIndication(base) else base }
    CompositionLocalProvider(LocalFeedback provides feedback, LocalIndication provides indication, content = content)
}

/**
 * Wraps the theme's indication (the ripple): the visuals are untouched, and
 * every press released inside its target also asks the engine for the default
 * tap. Presses that turn into drags are cancelled by Compose and never reach
 * here; long presses are ignored (they have their own feedback).
 */
class FeedbackIndication(private val base: IndicationNodeFactory) : IndicationNodeFactory {
    override fun create(interactionSource: InteractionSource): DelegatableNode = Node(interactionSource, base.create(interactionSource))

    override fun equals(other: Any?) = other is FeedbackIndication && other.base == base
    override fun hashCode() = base.hashCode() * 31 + 1

    private class Node(
        private val source: InteractionSource,
        private val inner: DelegatableNode,
    ) : DelegatingNode(), CompositionLocalConsumerModifierNode {
        init {
            delegate(inner)
        }

        override fun onAttach() {
            coroutineScope.launch {
                var pressedAt = 0L
                source.interactions.collect { interaction ->
                    when (interaction) {
                        is PressInteraction.Press -> pressedAt = System.nanoTime()
                        is PressInteraction.Release -> {
                            val held = (System.nanoTime() - pressedAt) / 1_000_000
                            (currentValueOf(LocalFeedback) as? TapFeedback)?.defaultTap(held)
                        }
                        else -> Unit
                    }
                }
            }
        }
    }
}

/** A click that answers with [haptic] and [cue] (null = none) before running [onClick]. */
fun Modifier.feedbackClickable(
    haptic: Haptic? = Haptic.Select,
    cue: Cue? = Cue.Tap,
    enabled: Boolean = true,
    onClickLabel: String? = null,
    role: Role? = null,
    onClick: () -> Unit,
): Modifier = composed {
    val fb = LocalFeedback.current
    val action = rememberUpdatedState(onClick)
    Modifier.foundationClickable(enabled = enabled, onClickLabel = onClickLabel, role = role) {
        fb.play(haptic, cue)
        action.value()
    }
}

/** Click and long click, the long click answering with [Haptic.LongPress]. */
@OptIn(ExperimentalFoundationApi::class)
fun Modifier.feedbackCombinedClickable(
    haptic: Haptic? = Haptic.Select,
    cue: Cue? = Cue.Tap,
    onLongClickHaptic: Haptic? = Haptic.LongPress,
    onLongClickCue: Cue? = null,
    enabled: Boolean = true,
    onLongClick: (() -> Unit)? = null,
    onClick: () -> Unit,
): Modifier = composed {
    val fb = LocalFeedback.current
    val click = rememberUpdatedState(onClick)
    val long = rememberUpdatedState(onLongClick)
    Modifier.combinedClickable(
        enabled = enabled,
        onLongClick = long.value?.let { { fb.play(onLongClickHaptic, onLongClickCue); long.value?.invoke() } },
        onClick = {
            fb.play(haptic, cue)
            click.value()
        },
    )
}

/** Play whichever of [haptic] / [cue] is non-null. */
fun Feedback.play(haptic: Haptic?, cue: Cue?, step: Int = 0) {
    haptic?.let(::haptic)
    cue?.let { cue(it, step) }
}

/** [action] preceded by feedback: for `onClick = ` parameters of stock controls. */
@Composable
fun feedbackAction(haptic: Haptic? = Haptic.Select, cue: Cue? = Cue.Tap, action: () -> Unit): () -> Unit {
    val fb = LocalFeedback.current
    val latest = rememberUpdatedState(action)
    return remember(fb, haptic, cue) { { fb.play(haptic, cue); latest.value() } }
}

/** `onCheckedChange` for a switch or checkbox: on and off answer differently. */
@Composable
fun toggleAction(onChange: (Boolean) -> Unit): (Boolean) -> Unit {
    val fb = LocalFeedback.current
    val latest = rememberUpdatedState(onChange)
    return remember(fb) {
        { on: Boolean ->
            fb.both(if (on) Haptic.ToggleOn else Haptic.ToggleOff, if (on) Cue.ToggleOn else Cue.ToggleOff)
            latest.value(on)
        }
    }
}

/** For a sheet, dialog or popover: [Cue.Open] as it appears and [Cue.Close] as it leaves. */
@Composable
fun OpenCloseFeedback(open: Cue = Cue.Open, close: Cue = Cue.Close) {
    val fb = LocalFeedback.current
    DisposableEffect(fb) {
        fb.cue(open)
        onDispose { fb.cue(close) }
    }
}
