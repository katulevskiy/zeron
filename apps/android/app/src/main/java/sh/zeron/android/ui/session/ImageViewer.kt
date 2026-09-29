package sh.zeron.android.ui.session

import android.graphics.Bitmap
import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import sh.zeron.android.design.CircleIconButton
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.GlyphKind
import sh.zeron.android.design.StatusGlyph

/** Full-screen attachment viewer: pinch/double-tap zoom, pan, tap or ✕ to close. */
@Composable
fun ImageViewer(initial: Bitmap?, load: suspend () -> Bitmap?, dismiss: () -> Unit) {
    var bitmap by remember { mutableStateOf(initial) }
    LaunchedEffect(Unit) { if (bitmap == null) bitmap = load() }
    val scale = remember { Animatable(1f) }
    var offset by remember { mutableStateOf(Offset.Zero) }
    var pinch by remember { mutableFloatStateOf(1f) }
    val scope = rememberCoroutineScope()
    val transform = rememberTransformableState { zoom, pan, _ ->
        pinch = (pinch * zoom).coerceIn(1f, 6f)
        scope.launch { scale.snapTo(pinch) }
        offset = if (pinch <= 1.01f) Offset.Zero else offset + pan
    }
    Box(
        Modifier
            .fillMaxSize()
            .background(Color.Black)
            .pointerInput(Unit) {
                detectTapGestures(
                    onTap = { dismiss() },
                    onDoubleTap = {
                        val target = if (pinch > 1.1f) 1f else 2.5f
                        pinch = target
                        if (target == 1f) offset = Offset.Zero
                        scope.launch { scale.animateTo(target) }
                    },
                )
            },
        contentAlignment = Alignment.Center,
    ) {
        val b = bitmap
        if (b == null) {
            StatusGlyph(GlyphKind.Spinner, size = 20.dp)
        } else {
            Image(
                b.asImageBitmap(),
                contentDescription = "Attachment",
                contentScale = ContentScale.Fit,
                modifier = Modifier
                    .fillMaxSize()
                    .transformable(transform)
                    .graphicsLayer {
                        scaleX = scale.value
                        scaleY = scale.value
                        translationX = offset.x
                        translationY = offset.y
                    },
            )
        }
        // Always light-on-dark: the viewer is black in both appearances.
        CircleIconButton(
            Glyph.Close,
            Modifier
                .align(Alignment.TopStart)
                .windowInsetsPadding(WindowInsets.systemBars)
                .padding(12.dp)
                .background(Color.White.copy(alpha = 0.16f), androidx.compose.foundation.shape.CircleShape),
            tint = Color.White,
            plate = false,
            label = "Close",
            onClick = dismiss,
        )
    }
}
