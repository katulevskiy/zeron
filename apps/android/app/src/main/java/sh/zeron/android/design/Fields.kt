package sh.zeron.android.design

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp

/** A filled single- or multi-line text field in Zeron's style (no Material). */
@Composable
fun ZTextField(
    value: String,
    onChange: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
    mono: Boolean = false,
    singleLine: Boolean = true,
    minLines: Int = 1,
    focus: FocusRequester? = null,
    keyboardType: KeyboardType = KeyboardType.Text,
    capitalization: KeyboardCapitalization = KeyboardCapitalization.None,
    imeAction: ImeAction = ImeAction.Done,
    onSubmit: (() -> Unit)? = null,
) {
    val c = LocalColors.current
    val style = if (mono) ZType.mono(14.5f) else ZType.sans(16f)
    Box(
        modifier
            .fillMaxWidth()
            .background(c.controlFill, RoundedCornerShape(14.dp))
            .padding(horizontal = 14.dp, vertical = 12.dp),
    ) {
        if (value.isEmpty()) ZText(placeholder, style, c.tertiary, maxLines = 1)
        BasicTextField(
            value, onChange,
            Modifier
                .fillMaxWidth()
                .then(if (focus != null) Modifier.focusRequester(focus) else Modifier),
            textStyle = style.copy(color = c.text),
            cursorBrush = SolidColor(c.accent),
            singleLine = singleLine,
            minLines = minLines,
            keyboardOptions = KeyboardOptions(capitalization = capitalization, keyboardType = keyboardType, imeAction = imeAction, autoCorrectEnabled = !mono),
            keyboardActions = KeyboardActions(onDone = { onSubmit?.invoke() }, onGo = { onSubmit?.invoke() }, onSearch = { onSubmit?.invoke() }),
        )
    }
}

/** The capsule search field (Search screen, pickers). */
@Composable
fun SearchField(
    value: String,
    onChange: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
    focus: FocusRequester? = null,
) {
    val c = LocalColors.current
    Row(
        modifier
            .fillMaxWidth()
            .height(44.dp)
            .background(c.controlFill, CircleShape)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        GlyphIcon(Glyph.Search, c.secondary, size = 17.dp)
        Spacer(Modifier.width(9.dp))
        Box(Modifier.weight(1f)) {
            if (value.isEmpty()) ZText(placeholder, ZType.sans(16f), c.tertiary, maxLines = 1)
            BasicTextField(
                value, onChange,
                Modifier
                    .fillMaxWidth()
                    .then(if (focus != null) Modifier.focusRequester(focus) else Modifier),
                textStyle = ZType.sans(16f).copy(color = c.text),
                cursorBrush = SolidColor(c.accent),
                singleLine = true,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            )
        }
        if (value.isNotEmpty()) {
            Spacer(Modifier.width(6.dp))
            Box(
                Modifier
                    .background(c.tertiary.copy(alpha = 0.35f), CircleShape)
                    .pressable(shape = CircleShape) { onChange("") }
                    .padding(3.dp),
            ) { GlyphIcon(Glyph.Close, c.background, size = 12.dp, weight = 2.4f) }
        }
    }
}

/** A thin determinate/indeterminate progress track (bootstrap, installs). */
@Composable
fun ProgressTrack(progress: Float?, modifier: Modifier = Modifier) {
    val c = LocalColors.current
    val phase = if (progress == null) {
        val t = rememberInfiniteTransition(label = "track")
        t.animateFloat(0f, 1f, infiniteRepeatable(tween(1100, easing = LinearEasing)), label = "sweep").value
    } else 0f
    Box(
        modifier
            .fillMaxWidth()
            .height(4.dp)
            .background(c.controlFill, CircleShape),
    ) {
        androidx.compose.foundation.Canvas(Modifier.matchParentSize()) {
            val r = size.height / 2
            if (progress != null) {
                drawRoundRect(c.accent, size = size.copy(width = size.width * progress.coerceIn(0.02f, 1f)), cornerRadius = androidx.compose.ui.geometry.CornerRadius(r))
            } else {
                val w = size.width * 0.3f
                val x = (size.width + w) * phase - w
                drawRoundRect(
                    c.accent,
                    topLeft = androidx.compose.ui.geometry.Offset(x.coerceAtLeast(0f), 0f),
                    size = size.copy(width = (minOf(x + w, size.width) - x.coerceAtLeast(0f)).coerceAtLeast(0f)),
                    cornerRadius = androidx.compose.ui.geometry.CornerRadius(r),
                )
            }
        }
    }
}
