package sh.zeron.android.ui

import android.content.Context
import android.content.Intent
import android.provider.Settings
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import sh.zeron.android.feedback.OpenCloseFeedback
import sh.zeron.android.feedback.tapAction

/** The phone's sound settings, or its settings home when a skin has no such screen. */
fun openSoundSettings(context: Context) {
    val flags = Intent.FLAG_ACTIVITY_NEW_TASK
    runCatching { context.startActivity(Intent(Settings.ACTION_SOUND_SETTINGS).addFlags(flags)) }
        .onFailure { runCatching { context.startActivity(Intent(Settings.ACTION_SETTINGS).addFlags(flags)) } }
}

/**
 * What to do when the phone's "Touch sounds" setting is off: how to turn it on (the menu names differ by maker),
 * a shortcut to the sound settings, and "play anyway", which makes Zeron ignore that setting for its own
 * interface sounds. Zeron plays them itself (as sonification, not through the system's touch-sound path), so the
 * setting does not technically block them: it is honoured by default as a courtesy.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun TouchSoundsSheet(playAnyway: Boolean, onPlayAnyway: (Boolean) -> Unit, onDismiss: () -> Unit) {
    val context = LocalContext.current
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = MaterialTheme.colorScheme.surfaceContainerLow,
    ) {
        OpenCloseFeedback()
        Column(Modifier.padding(horizontal = 24.dp).padding(bottom = 24.dp).navigationBarsPadding().verticalScroll(rememberScrollState())) {
            Text("Touch sounds are off", style = MaterialTheme.typography.headlineSmallEmphasized)
            Spacer(Modifier.height(12.dp))
            Text(
                "Your phone's \"Touch sounds\" setting is off, so Zeron keeps its interface sounds (taps, switches, menus) quiet to match. " +
                    "Session chimes are not affected.",
                style = MaterialTheme.typography.bodyMedium,
            )
            Spacer(Modifier.height(16.dp))
            Text("To turn them on", style = MaterialTheme.typography.titleSmall)
            Spacer(Modifier.height(6.dp))
            Step("Pixel and most phones", "Settings > Sound & vibration > Touch sounds. For vibration, also Vibration & haptics > Touch feedback.")
            Step("Samsung", "Settings > Sounds and vibration > System sound/vibration control > Touch sounds.")
            Spacer(Modifier.height(8.dp))
            Text(
                "Or skip the phone setting: Zeron plays these sounds itself, so it can play them anyway. " +
                    "They still follow your volume, silent mode and Do Not Disturb.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(20.dp))
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(
                    onClick = tapAction { openSoundSettings(context) },
                    modifier = Modifier.fillMaxWidth(),
                    shapes = ButtonDefaults.shapes(),
                ) { Text("Open sound settings") }
                if (playAnyway) {
                    OutlinedButton(
                        onClick = tapAction { onPlayAnyway(false) },
                        modifier = Modifier.fillMaxWidth(),
                        shapes = ButtonDefaults.shapes(),
                    ) { Text("Playing anyway. Tap to follow the phone again") }
                } else {
                    OutlinedButton(
                        onClick = tapAction { onPlayAnyway(true) },
                        modifier = Modifier.fillMaxWidth(),
                        shapes = ButtonDefaults.shapes(),
                    ) { Text("Play sounds anyway") }
                }
            }
        }
    }
}

@Composable
private fun Step(where: String, how: String) {
    Column(Modifier.padding(vertical = 4.dp)) {
        Text(where, style = MaterialTheme.typography.labelLarge)
        Text(how, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}
