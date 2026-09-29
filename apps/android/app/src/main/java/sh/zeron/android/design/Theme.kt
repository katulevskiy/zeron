package sh.zeron.android.design

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.text.selection.LocalTextSelectionColors
import androidx.compose.foundation.text.selection.TextSelectionColors
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember

/** User appearance choice (Settings → Appearance). */
enum class Appearance { System, Light, Dark }

@Composable
fun ZeronTheme(appearance: Appearance, overlays: Overlays, content: @Composable () -> Unit) {
    val dark = when (appearance) {
        Appearance.System -> isSystemInDarkTheme()
        Appearance.Light -> false
        Appearance.Dark -> true
    }
    val colors = if (dark) ZeronColors.Dark else ZeronColors.Light
    val selection = remember(colors) { TextSelectionColors(colors.accent, colors.accent.copy(alpha = 0.3f)) }
    CompositionLocalProvider(
        LocalColors provides colors,
        LocalOverlays provides overlays,
        LocalTextSelectionColors provides selection,
        content = content,
    )
}
