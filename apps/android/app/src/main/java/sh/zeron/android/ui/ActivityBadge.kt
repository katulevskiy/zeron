package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.toShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.text.PlatformTextStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.constrainWidth
import androidx.compose.ui.unit.constrainHeight
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
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

/** More running children add scallops; the exact, capped count stays readable. */
@Composable
private fun SubagentCountBadge(count: UInt, modifier: Modifier = Modifier) {
    val label = SessionActivity.badgeLabel(count) ?: return
    val polygon = when {
        count <= 4u -> MaterialShapes.Cookie4Sided
        count <= 6u -> MaterialShapes.Cookie6Sided
        count <= 7u -> MaterialShapes.Cookie7Sided
        count <= 9u -> MaterialShapes.Cookie9Sided
        else -> MaterialShapes.Cookie12Sided
    }
    Box(
        modifier.defaultMinSize(minWidth = 28.dp, minHeight = 28.dp)
            .background(if (LocalDarkTheme.current) Color(0xFF7C61DB) else Color(0xFF5B43E8), polygon.toShape())
            .semantics(mergeDescendants = true) {
                contentDescription = if (count == 1u) "1 subagent running" else "$count subagents running"
            }
            .padding(horizontal = 7.dp, vertical = 5.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            label,
            color = Color.White,
            maxLines = 1,
            softWrap = false,
            style = MaterialTheme.typography.labelLarge.copy(
                fontSize = 16.sp,
                lineHeight = 18.sp,
                fontWeight = FontWeight.Bold,
                fontFeatureSettings = "tnum",
                textAlign = TextAlign.Center,
                platformStyle = PlatformTextStyle(includeFontPadding = false),
            ),
        )
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
        val badgeHeight = maxOf(28.dp.roundToPx(), badge?.height ?: 0)
        val width = constraints.constrainWidth(maxOf(56.dp.roundToPx(), badge?.width ?: 0))
        val tileTop = badgeHeight - 12.dp.roundToPx()
        val height = constraints.constrainHeight(tileTop + tile.height)
        layout(width, height) {
            tile.placeRelative(0, tileTop)
            badge?.placeRelative(width - badge.width, 0)
        }
    }
}

/** All requested counts, using the production tile and the actual app theme. */
@Preview(showBackground = true)
@Composable
fun ActivityBadgePreview() {
    ZeronTheme {
        Surface {
            Column(Modifier.padding(start = 24.dp, end = 24.dp, top = 112.dp, bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(24.dp)) {
                Text("Running subagents", style = MaterialTheme.typography.titleLarge)
                Text("More agents, more scallops", style = MaterialTheme.typography.bodyMedium)
                listOf(1u, 2u, 3u, 4u, 5u, 6u, 7u, 8u, 9u, 10u, 11u, 12u, 67u, 99u, 100u, 0u).chunked(4).forEach { counts ->
                    Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                        counts.forEach { count ->
                            Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                HarnessActivityTile("claude-code", Color(0xFFAB6092), count)
                                Text(if (count == 0u) "Hidden" else if (count > 99u) "100+" else "$count", style = MaterialTheme.typography.labelMedium)
                            }
                        }
                    }
                }
            }
        }
    }
}
