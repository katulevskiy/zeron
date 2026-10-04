package sh.zeron.android.voice.assistant

import android.provider.Settings
import androidx.compose.animation.core.*
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.*
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.semantics.*
import sh.zeron.android.voice.JarvisOrb
import sh.zeron.android.voice.JarvisState
import uniffi.zeron_core.VoiceOrb
import kotlin.math.*

@Composable
internal fun JarvisCameraStage(
    state: JarvisState,
    camera: OverlayCameraState,
    preview: @Composable () -> Unit,
    onAction: (AssistantAction) -> Unit,
) {
    val context = LocalContext.current
    val reduced = Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    val duration = if (reduced) 0 else 280
    val diameter by animateDpAsState(if (camera.open) 252.dp else 184.dp, tween(duration), label = "camera morph")
    val reveal by animateFloatAsState(if (camera.ready) 1f else 0f, tween(duration), label = "camera iris")
    var irisFinished by remember { mutableStateOf(false) }
    LaunchedEffect(camera.ready) {
        irisFinished = false
        if (camera.ready) {
            kotlinx.coroutines.delay((duration + 32).toLong())
            irisFinished = true
        }
    }
    val purple = Color(0xFF8B7CF6)
    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Box(Modifier.size(diameter).clip(CircleShape), contentAlignment = Alignment.Center) {
            if (camera.open) {
                Box(Modifier.fillMaxSize().background(Color(0xFF171024)).semantics {
                    contentDescription = "Camera preview"
                    stateDescription = if (camera.ready && irisFinished) "Live" else "Opening"
                }) { preview() }
                // Six iris blades open over the live preview only after the
                // first actual frame, so loading never flashes an empty view.
                if (!irisFinished) Canvas(Modifier.fillMaxSize()) {
                    val center = Offset(size.width / 2, size.height / 2)
                    val outer = size.minDimension * 0.76f
                    val inner = size.minDimension * 0.72f * reveal
                    for (i in 0..5) {
                        val a = (i * PI / 3).toFloat()
                        fun point(r: Float, angle: Float) = center + Offset(cos(angle) * r, sin(angle) * r)
                        val path = Path().apply {
                            val p0 = point(inner, a)
                            moveTo(p0.x, p0.y)
                            val p1 = point(outer, a - .25f); lineTo(p1.x, p1.y)
                            val p2 = point(outer, a + 1.08f); lineTo(p2.x, p2.y)
                            val p3 = point(inner, a + 1.0472f); lineTo(p3.x, p3.y)
                            close()
                        }
                        drawPath(path, Color(0xFF171024).copy(alpha = 1 - reveal))
                        if (reveal < 1f) drawPath(path, purple.copy(alpha = .25f * (1 - reveal)), style = Stroke(1.dp.toPx()))
                    }
                }
                if (!camera.ready) JarvisOrb(VoiceOrb.CONNECTING, 0f, 0f, Modifier.size(184.dp), tint = purple)
                if (camera.capturing) Box(Modifier.fillMaxSize().background(Color.White.copy(alpha = .22f)))
                Box(Modifier.fillMaxSize().border(2.dp, purple.copy(alpha = .75f), CircleShape))
            } else JarvisOrb(if (state.muted) VoiceOrb.MUTED else state.call?.orb ?: VoiceOrb.IDLE,
                state.call?.microphone ?: 0f, state.call?.speaker ?: 0f, Modifier.fillMaxSize(), tint = purple)
        }
        if (camera.open) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(24.dp)) {
                IconButton(onClick = { onAction(AssistantAction.CloseCamera) }, modifier = Modifier.size(48.dp)) {
                    Icon(Icons.Default.Close, "Close camera", tint = MaterialTheme.colorScheme.onSurface)
                }
                FilledIconButton(onClick = { onAction(AssistantAction.Shutter) }, enabled = camera.ready && !camera.capturing,
                    modifier = Modifier.size(64.dp).semantics { contentDescription = "Shutter" }, colors = IconButtonDefaults.filledIconButtonColors(containerColor = Color.White, contentColor = purple)) {
                    Canvas(Modifier.size(46.dp).border(2.dp, purple, CircleShape)) { drawCircle(purple, size.minDimension * .42f) }
                    // Native accessibility receives a named 64dp shutter.
                }
                Spacer(Modifier.size(48.dp))
            }
        }
    }
}
