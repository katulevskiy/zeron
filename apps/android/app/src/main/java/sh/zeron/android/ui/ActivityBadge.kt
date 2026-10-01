package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Badge
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.SessionActivity
import sh.zeron.android.design.HarnessMark
import sh.zeron.android.design.LocalDarkTheme

@Composable
fun sessionActivityColor(shape: SessionActivity.Shape): Color = when (shape) {
    SessionActivity.Shape.MainRunning -> if (LocalDarkTheme.current) Color(0xFFAB9AFF) else Color(0xFF5B43E8)
    SessionActivity.Shape.SubagentsRunning -> if (LocalDarkTheme.current) Color(0xFFFACC45) else Color(0xFFB88700)
    SessionActivity.Shape.CallbackWaiting -> if (LocalDarkTheme.current) Color(0xFF8AB4FF) else Color(0xFF2967D8)
}

/** Material's capsule grows with its text and centers the count on both axes. */
@Composable
private fun SubagentCountBadge(count: UInt, modifier: Modifier) {
    val label = SessionActivity.badgeLabel(count) ?: return
    Badge(
        containerColor = if (LocalDarkTheme.current) Color(0xFF7C61DB) else Color(0xFF5B43E8),
        contentColor = Color.White,
        modifier = modifier.defaultMinSize(minWidth = 22.dp, minHeight = 22.dp).semantics {
            contentDescription = if (count == 1u) "1 subagent running" else "$count subagents running"
        },
    ) {
        Text(
            label,
            modifier = Modifier.padding(horizontal = 4.dp),
            style = MaterialTheme.typography.labelSmall.copy(fontWeight = FontWeight.SemiBold, fontFeatureSettings = "tnum"),
        )
    }
}

@Composable
fun HarnessActivityTile(harness: String?, tone: Color, running: UInt) {
    Box {
        Box(
            Modifier.size(48.dp).clip(RoundedCornerShape(16.dp)).background(tone.copy(alpha = if (LocalDarkTheme.current) .18f else .12f)),
            contentAlignment = Alignment.Center,
        ) { HarnessMark(harness, 24.dp, tint = MaterialTheme.colorScheme.onSurface) }
        if (running > 0u) {
            // Keep the right edge fixed as the capsule grows inward. Even
            // "99+" leaves the row's headline clear. The vertical overhang
            // fits inside a grouped row's padding so its top cannot clip.
            SubagentCountBadge(running, Modifier.align(Alignment.TopEnd).offset(x = 8.dp, y = (-8).dp))
        }
    }
}

@Preview(showBackground = true)
@Composable
fun ActivityBadgePreview() {
    MaterialTheme {
        Surface {
            Column(Modifier.padding(start = 24.dp, end = 24.dp, top = 128.dp, bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(20.dp)) {
                Text("Running subagents", style = MaterialTheme.typography.titleMedium)
                listOf(0u, 1u, 9u, 10u, 99u, 100u).forEach { count ->
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(24.dp)) {
                        HarnessActivityTile("claude-code", Color(0xFFAB6092), count)
                        Text("$count → ${SessionActivity.badgeLabel(count) ?: "hidden"}")
                    }
                }
            }
        }
    }
}
