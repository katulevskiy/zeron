package sh.zeron.android.design

import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import uniffi.zeron_core.ColorRole

/**
 * Zeron Light / Zeron Dark (crates/theme builtins, same values as the iOS
 * `Palette`): cool neutrals, one violet accent. Paint only — nothing here
 * affects layout, so a theme switch never relayouts the transcript.
 *
 * Every color exists both as an ARGB int (the transcript painter draws with
 * android.graphics) and as a Compose [Color].
 */
@Immutable
class ZeronColors(val dark: Boolean) {
    private fun pick(light: Long, dark: Long, alpha: Float = 1f): Int =
        argb(if (this.dark) dark else light, alpha)

    val backgroundArgb = pick(0xF3F3F5, 0x060606)
    val background = Color(backgroundArgb)
    val elevated = Color(pick(0xFFFFFF, 0x111113))
    val text = Color(pick(0x27272C, 0xE8E8EA))
    val secondary = Color(pick(0x62626A, 0xA9A9AE))
    val tertiary = Color(pick(0x97979F, 0x6B6B72))
    val hairline = Color(pick(0xE2E2E6, 0x1E1E22))
    val accent = Color(pick(0x5B43E8, 0x8B7CF6))
    val accentSoft = Color(pick(0x5B43E8, 0x8B7CF6, 0.12f))
    /** Translucent control fill that reads on glass in both modes. */
    val controlFill = Color(pick(0x27272C, 0xE8E8EA, 0.075f))
    val danger = Color(pick(0xDC2626, 0xF87171))
    val success = Color(pick(0x15803D, 0x34D399))
    val warning = Color(pick(0xA16207, 0xFACC15))
    val userBubble = Color(pick(0xFFFFFF, 0x19191C))
    val chip = Color(pick(0xE7E7EB, 0x1C1C20))
    val rowActive = if (dark) Color.White.copy(alpha = 0.06f) else Color.White.copy(alpha = 0.9f)
    /** Desktop sidebar "subline": `text_muted` @ 0.5 (project label, branch). */
    val subline = Color(pick(0x62626A, 0xA9A9AE, 0.5f))

    /**
     * Android has no Liquid Glass: floating chrome is a near-opaque elevated
     * plate with a hairline edge and a soft shadow — reads as "above the
     * content" without a costly backdrop blur.
     */
    val glass = if (dark) Color(0xFC1C1C1F.toInt()) else Color(0xFDFFFFFF.toInt())
    val glassEdge = if (dark) Color.White.copy(alpha = 0.08f) else Color.Black.copy(alpha = 0.07f)
    val scrim = Color.Black.copy(alpha = if (dark) 0.55f else 0.32f)

    // Status tones, as the desktop sidebar tints them (`status_dot_color`).
    val statusWorking = Color(pick(0x5B43E8, 0x8B7CF6, 0.55f))
    val statusInput = Color(pick(0x5B43E8, 0x8B7CF6, 0.6f))
    val statusFailed = Color(pick(0xDC2626, 0xF87171, 0.65f))
    val statusDone = Color(pick(0x15803D, 0x34D399, 0.9f))
    val statusIdle = Color(pick(0x000000, 0xFFFFFF, 0.14f))
    val statusTime = Color(pick(0x62626A, 0xA9A9AE, 0.5f))

    /** Mini spinner rows (violet ramp) and the transcript trailer's pastels. */
    val glyphTints = listOf(
        Color(pick(0x7965EC, 0xABA1F9)),
        Color(pick(0x5B43E8, 0x8B7CF6)),
        Color(pick(0x4332AC, 0x7266CA)),
    )
    val trailerTints = listOf(Color(0xFFB6D3EF), Color(0xFFEDB185), Color(0xFFF888A0))

    /** Project tones (desktop project_icon.rs): slate, blue, violet, rose, amber, emerald, teal, orange. */
    private val projectTones = listOf(
        0x475569L to 0x94A3B8L, 0x2563EBL to 0x93C5FDL, 0x7C3AEDL to 0xC4B5FDL,
        0xBE123CL to 0xFDA4AFL, 0xA16207L to 0xFCD34DL, 0x047857L to 0x6EE7B7L,
        0x0F766EL to 0x5EEAD4L, 0xC2410CL to 0xFDBA74L,
    ).map { (l, d) -> Color(pick(l, d)) }

    fun project(index: Int): Color = projectTones[Math.floorMod(index, projectTones.size)]

    /** Brand tint for harness marks that keep their own color (Claude orange). */
    fun brand(harness: String?): Color? = when (harness) {
        "claude-code", "mock", null -> Color(0xFFD97757)
        else -> null
    }

    private val roles = IntArray(ColorRole.entries.size).also { table ->
        for (role in ColorRole.entries) table[role.ordinal] = resolve(role)
    }

    /** Transcript paint role → ARGB (precomputed; called per run while painting). */
    fun role(role: ColorRole): Int = roles[role.ordinal]

    private fun resolve(role: ColorRole): Int = when (role) {
        ColorRole.TEXT -> pick(0x27272C, 0xE8E8EA)
        ColorRole.TEXT_SECONDARY -> pick(0x62626A, 0xA9A9AE)
        ColorRole.TEXT_TERTIARY -> pick(0x97979F, 0x6B6B72)
        ColorRole.LINK, ColorRole.ACCENT -> pick(0x5B43E8, 0x8B7CF6)
        ColorRole.DANGER -> pick(0xDC2626, 0xF87171)
        ColorRole.SUCCESS -> pick(0x15803D, 0x34D399)
        ColorRole.WARNING -> pick(0xA16207, 0xFACC15)
        ColorRole.INLINE_CODE_TEXT -> pick(0x3F3F46, 0xDCDCE0)
        ColorRole.INLINE_CODE_BACKGROUND -> pick(0xE9E9ED, 0x1A1A1E)
        ColorRole.CODE_TEXT -> pick(0x303035, 0xE8E8EA)
        ColorRole.CODE_BACKGROUND -> pick(0xFAFAFB, 0x0B0B0D)
        ColorRole.CODE_BORDER -> pick(0xE4E4E8, 0x1F1F23)
        ColorRole.QUOTE_BAR -> pick(0x5B43E8, 0x8B7CF6, 0.45f)
        ColorRole.RULE -> pick(0xE2E2E6, 0x1E1E22)
        ColorRole.TABLE_BORDER -> pick(0xE2E2E6, 0x232327)
        ColorRole.TABLE_HEADER_BACKGROUND -> pick(0xF3F3F5, 0x121215)
        ColorRole.USER_BUBBLE -> pick(0xFFFFFF, 0x19191C)
        ColorRole.CHIP_BACKGROUND -> pick(0xE7E7EB, 0x1C1C20)
        ColorRole.SYNTAX_KEYWORD -> pick(0x5B43E8, 0x8B7CF6)
        ColorRole.SYNTAX_STRING -> pick(0x15803D, 0x34D399)
        ColorRole.SYNTAX_COMMENT -> pick(0x6B7280, 0x92929A)
        ColorRole.SYNTAX_NUMBER, ColorRole.SYNTAX_CONSTANT -> pick(0xA16207, 0xFACC15)
        ColorRole.SYNTAX_FUNCTION -> pick(0x2563EB, 0x60A5FA)
        ColorRole.SYNTAX_TYPE -> pick(0x7E22CE, 0xC084FC)
        ColorRole.SYNTAX_VARIABLE -> pick(0x303035, 0xE8E8EA)
        ColorRole.SYNTAX_PROPERTY -> pick(0x0E7490, 0x22D3EE)
        ColorRole.SYNTAX_OPERATOR, ColorRole.SYNTAX_PUNCTUATION -> pick(0x52525B, 0xA1A1AA)
        ColorRole.SYNTAX_TAG -> pick(0xBE185D, 0xF472B6)
        ColorRole.SYNTAX_ATTRIBUTE -> pick(0xB91C1C, 0xF87171)
        ColorRole.SYNTAX_ESCAPE -> pick(0x0E7490, 0x22D3EE)
        ColorRole.TEXT_FAINT -> pick(0x797981, 0x85858A)
        ColorRole.TEXT_SOFT -> pick(0x303035, 0xE8E8EA, 0.85f)
        ColorRole.TOOL_RAIL -> if (dark) argb(0xFFFFFF, 0.12f) else argb(0x000000, 0.162f)
        ColorRole.TOOL_BADGE -> if (dark) argb(0xFFFFFF, 0.06f) else argb(0x000000, 0.06f)
        ColorRole.TOOL_WELL -> if (dark) argb(0x000000, 0.16f) else argb(0xFFFFFF, 0.16f)
        ColorRole.AGENT_CARD -> if (dark) argb(0xFFFFFF, 0.03f) else argb(0x000000, 0.03f)
        ColorRole.AGENT_CARD_BORDER -> if (dark) argb(0xFFFFFF, 0.07f) else argb(0x000000, 0.0945f)
        ColorRole.AGENT_TILE -> if (dark) argb(0xFFFFFF, 0.08f) else argb(0x000000, 0.08f)
        ColorRole.DIFF_ADD_WASH -> pick(0x15803D, 0x34D399, 0.055f)
        ColorRole.DIFF_DEL_WASH -> pick(0xDC2626, 0xF87171, 0.055f)
        ColorRole.DIFF_ADD_BAR -> pick(0x15803D, 0x34D399, 0.55f)
        ColorRole.DIFF_DEL_BAR -> pick(0xDC2626, 0xF87171, 0.55f)
        ColorRole.DIFF_HUNK -> if (dark) argb(0x8B7CF6, 0.08f) else argb(0x5B43E8, 0.07f)
    }

    companion object {
        fun argb(rgb: Long, alpha: Float = 1f): Int =
            ((alpha.coerceIn(0f, 1f) * 255f + 0.5f).toInt() shl 24) or (rgb and 0xFFFFFF).toInt()

        val Light = ZeronColors(dark = false)
        val Dark = ZeronColors(dark = true)
    }
}

val LocalColors = staticCompositionLocalOf { ZeronColors.Dark }
