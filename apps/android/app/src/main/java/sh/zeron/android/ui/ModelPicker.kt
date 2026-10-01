package sh.zeron.android.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.FavoriteModel
import sh.zeron.android.design.LocalDarkTheme

/** The desktop's warning amber (favorite stars, filling context). */
@Composable
fun warningColor(): Color = if (LocalDarkTheme.current) Color(0xFFFBBF24) else Color(0xFFB45309)

// Interim list (replaced by the compact picker in the next commit).
@Composable
fun ModelPickerPopover(
    expanded: Boolean,
    onDismiss: () -> Unit,
    catalog: List<ModelChoice>,
    current: ModelChoice?,
    favorites: List<FavoriteModel>,
    onToggleFavorite: (FavoriteModel) -> Unit,
    onPick: (ModelChoice) -> Unit,
    locked: Boolean = false,
    loading: Boolean = false,
    labelFor: (String) -> String = { it },
) {
    AnchoredPopover(expanded, onDismiss) {
        for (e in ModelPickerRules.entries(catalog, favorites, "", current, locked)) {
            Text(e.choice.model.label, Modifier.fillMaxWidth().clickable { onPick(e.choice); onDismiss() }.padding(16.dp))
        }
    }
}
