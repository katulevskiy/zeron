package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.AllInclusive
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.text.PlatformTextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.constrainHeight
import androidx.compose.ui.unit.constrainWidth
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.ceil
import kotlin.math.max
import sh.zeron.android.core.SessionActivity
import sh.zeron.android.design.HarnessMark
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.design.ZeronTheme

@Composable
fun sessionActivityColor(shape: SessionActivity.Shape): Color = when (shape) {
    SessionActivity.Shape.MainRunning -> if (LocalDarkTheme.current) Color(0xFFAB9AFF) else Color(0xFF5B43E8)
    SessionActivity.Shape.SubagentsRunning -> if (LocalDarkTheme.current) Color(0xFFFACC45) else Color(0xFFB88700)
    SessionActivity.Shape.CallbackWaiting -> if (LocalDarkTheme.current) Color(0xFF8AB4FF) else Color(0xFF2967D8)
}

/** Canonical Material shapes keep their proportions, even for wide labels. */
@Composable
private fun SubagentCountBadge(count: UInt, modifier: Modifier = Modifier) {
    if (count == 0u) return
    val label = SessionActivity.badgeLabel(count)
    val polygon = when (count) {
        1u -> MaterialShapes.Pill
        2u -> MaterialShapes.Arch
        3u -> MaterialShapes.Triangle
        4u -> MaterialShapes.Diamond
        5u -> MaterialShapes.Pentagon
        6u -> MaterialShapes.Gem
        7u -> MaterialShapes.Cookie7Sided
        8u -> MaterialShapes.Clover8Leaf
        9u -> MaterialShapes.PuffyDiamond
        in 10u..20u -> MaterialShapes.ClamShell
        in 21u..99u -> MaterialShapes.Puffy
        else -> MaterialShapes.Heart
    }
    val iconSize = with(LocalDensity.current) { 24.sp.toDp() }
    Layout(
        modifier = modifier
            .background(if (LocalDarkTheme.current) Color(0xFF7C61DB) else Color(0xFF5B43E8), polygon.toShape())
            .clearAndSetSemantics {
                // The overflow icon is visual shorthand; announce the real count.
                contentDescription = if (count == 1u) "1 subagent running" else "$count subagents running"
            },
        content = {
            if (label == null) {
                Icon(Icons.Rounded.AllInclusive, contentDescription = null, modifier = Modifier.size(iconSize), tint = Color.White)
            } else {
                Text(
                    label,
                    color = Color.White,
                    maxLines = 1,
                    softWrap = false,
                    style = MaterialTheme.typography.labelLarge.copy(
                        fontSize = 20.sp,
                        lineHeight = 22.sp,
                        fontWeight = FontWeight.Bold,
                        fontFeatureSettings = "tnum",
                        textAlign = TextAlign.Center,
                        platformStyle = PlatformTextStyle(includeFontPadding = false),
                    ),
                )
            }
        },
    ) { measurables, _ ->
        val content = measurables.single().measure(Constraints())
        // Material's polygons retain their native aspect ratio inside a square
        // viewport. Measure the label, then grow this viewport uniformly; never
        // stretch a polygon to the label's rectangular bounds.
        val side = max(36.dp.roundToPx(), ceil(max(content.width / .62f, content.height / .72f)).toInt())
        layout(side, side) {
            content.placeRelative((side - content.width) / 2, (side - content.height) / 2)
        }
    }
}

@Composable
fun HarnessActivityTile(harness: String?, tone: Color, running: UInt) {
    // Measure the badge first: larger accessibility text grows the leading area
    // instead of clipping. Only the tile's empty top margin sits under the badge.
    Layout(content = {
        Box(
            Modifier.size(48.dp).clip(RoundedCornerShape(16.dp)).background(tone.copy(alpha = if (LocalDarkTheme.current) .18f else .12f)),
            contentAlignment = Alignment.Center,
        ) { HarnessMark(harness, 24.dp, tint = MaterialTheme.colorScheme.onSurface) }
        if (running > 0u) SubagentCountBadge(running)
    }) { measurables, constraints ->
        val tile = measurables[0].measure(Constraints.fixed(48.dp.roundToPx(), 48.dp.roundToPx()))
        val badge = measurables.getOrNull(1)?.measure(Constraints())
        val badgeHeight = maxOf(36.dp.roundToPx(), badge?.height ?: 0)
        val badgeLeft = tile.width - (badge?.width ?: 0) / 2
        val width = constraints.constrainWidth(maxOf(68.dp.roundToPx(), badgeLeft + (badge?.width ?: 0)))
        val tileTop = badgeHeight - 12.dp.roundToPx()
        val height = constraints.constrainHeight(tileTop + tile.height)
        layout(width, height) {
            tile.placeRelative(0, tileTop)
            badge?.placeRelative(badgeLeft, 0)
        }
    }
}

/** All requested counts, using the production tile and the actual app theme. */
@Preview(showBackground = true)
@Composable
fun ActivityBadgePreview() {
    ZeronTheme {
        Surface {
            Column(Modifier.verticalScroll(rememberScrollState()).padding(start = 24.dp, end = 24.dp, top = 112.dp, bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(20.dp)) {
                Text("Running subagents", style = MaterialTheme.typography.titleLarge)
                Text("Material shapes · natural proportions", style = MaterialTheme.typography.bodyMedium)
                listOf(1u, 2u, 3u, 4u, 5u, 6u, 7u, 8u, 9u, 10u, 11u, 12u, 20u, 21u, 67u, 99u, 100u, UInt.MAX_VALUE, 0u).chunked(4).forEach { counts ->
                    Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                        counts.forEach { count ->
                            Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                HarnessActivityTile("claude-code", Color(0xFFAB6092), count)
                                Text(if (count == 0u) "Hidden" else if (count == UInt.MAX_VALUE) "Max" else "$count", style = MaterialTheme.typography.labelMedium)
                            }
                        }
                        repeat(4 - counts.size) { Box(Modifier.weight(1f)) }
                    }
                }
            }
        }
    }
}
