package sh.zeron.android.design

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Matrix
import android.graphics.Paint
import android.graphics.Path
import android.graphics.PorterDuff
import android.graphics.PorterDuffColorFilter
import android.graphics.RectF
import android.util.LruCache
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.asComposePath
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Fill
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.core.graphics.PathParser
import com.caverock.androidsvg.SVG

/**
 * UI glyphs (the SF Symbols the iOS app leans on), drawn from path data on a
 * 24-unit grid with 1.6-unit round strokes — one weight across the app.
 */
enum class Glyph(val d: String, val fill: Boolean = false) {
    Plus("M12 5v14M5 12h14"),
    ArrowUp("M12 19V5M6 11l6-6 6 6"),
    ArrowDown("M12 5v14M6 13l6 6 6-6"),
    Stop("M8.5 7h7A1.5 1.5 0 0 1 17 8.5v7a1.5 1.5 0 0 1-1.5 1.5h-7A1.5 1.5 0 0 1 7 15.5v-7A1.5 1.5 0 0 1 8.5 7z", fill = true),
    Ellipsis("M4.4 12a1.6 1.6 0 1 0 3.2 0a1.6 1.6 0 1 0-3.2 0zM10.4 12a1.6 1.6 0 1 0 3.2 0a1.6 1.6 0 1 0-3.2 0zM16.4 12a1.6 1.6 0 1 0 3.2 0a1.6 1.6 0 1 0-3.2 0z", fill = true),
    ChevronRight("M9.5 5.5l6.5 6.5-6.5 6.5"),
    ChevronLeft("M14.5 5.5L8 12l6.5 6.5"),
    ChevronDown("M5.5 9.5L12 16l6.5-6.5"),
    Close("M6.5 6.5l11 11M17.5 6.5l-11 11"),
    Check("M5 12.5l4.5 4.5L19 7.5"),
    Search("M10.5 17.5a7 7 0 1 0 0-14 7 7 0 0 0 0 14zM15.8 15.8L20.5 20.5"),
    Folder("M3.5 7.5A1.5 1.5 0 0 1 5 6h4.2l2 2H19a1.5 1.5 0 0 1 1.5 1.5v8A1.5 1.5 0 0 1 19 19H5a1.5 1.5 0 0 1-1.5-1.5z"),
    FolderPlus("M3.5 7.5A1.5 1.5 0 0 1 5 6h4.2l2 2H19a1.5 1.5 0 0 1 1.5 1.5v8A1.5 1.5 0 0 1 19 19H5a1.5 1.5 0 0 1-1.5-1.5zM12 10.5v5.5M9.25 13.25h5.5"),
    Pin("M9 3.5h6l-1 5.5 3 3V14H7v-2l3-3zM12 14v6.5"),
    Archive("M3.5 4.5h17v4.5h-17zM5 9v9.5A1.5 1.5 0 0 0 6.5 20h11a1.5 1.5 0 0 0 1.5-1.5V9M10 13h4"),
    Unarchive("M3.5 4.5h17v4.5h-17zM5 9v9.5A1.5 1.5 0 0 0 6.5 20h11a1.5 1.5 0 0 0 1.5-1.5V9M12 17.5v-6M9.5 14l2.5-2.5 2.5 2.5"),
    Trash("M4 7h16M9 7V4.5h6V7M6 7l1 12.5A1.5 1.5 0 0 0 8.5 21h7a1.5 1.5 0 0 0 1.5-1.5L18 7M10 11v6M14 11v6"),
    Pencil("M4.5 19.5l1-4L16 5l3 3L8.5 18.5zM13.5 7.5l3 3"),
    Copy("M8.5 8.5h10v10h-10zM5.5 15.5v-10h10"),
    Clock("M12 20.5a8.5 8.5 0 1 0 0-17 8.5 8.5 0 0 0 0 17zM12 7.5v4.5l3 2"),
    Bell("M6 16v-5a6 6 0 1 1 12 0v5l1.5 2h-15zM10 20.5a2 2 0 0 0 4 0"),
    Desktop("M3.5 5h17v11h-17zM8 20h8M12 16v4"),
    Phone("M7.5 3h9v18h-9zM11 18h2"),
    Photo("M4 5h16v14H4zM4 16l5-5 4 4 2-2 5 5M15.5 8.5a1 1 0 1 0 0 2 1 1 0 0 0 0-2z"),
    Sparkle("M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9z"),
    Link("M10 14a4 4 0 0 0 5.66 0l3-3a4 4 0 0 0-5.66-5.66l-1 1M14 10a4 4 0 0 0-5.66 0l-3 3a4 4 0 0 0 5.66 5.66l1-1"),
    Refresh("M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v4.5H15"),
    Power("M12 3.5v8M6.7 6.7a7.5 7.5 0 1 0 10.6 0"),
    Download("M12 4v11M7 10l5 5 5-5M5 20h14"),
    Square("M6.5 5h11A1.5 1.5 0 0 1 19 6.5v11a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 5 17.5v-11A1.5 1.5 0 0 1 6.5 5z"),
    Warning("M12 4l9 16H3zM12 10v4.5M12 17.25v.25"),
    Question("M4 5h16v11H9.5L5.5 20v-4H4zM10 9a2 2 0 1 1 2.6 1.9c-.35.12-.6.45-.6.82v.28M12 13.9v.2"),
    Tray("M4 13l2.5-8h11L20 13v6H4zM4 13h4.5l1 2h5l1-2H20"),
    Person("M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4.5 20.5a7.5 7.5 0 0 1 15 0"),
    Sun("M12 7.5a4.5 4.5 0 1 0 0 9 4.5 4.5 0 0 0 0-9zM12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"),
    Moon("M19.5 14.5A7.5 7.5 0 1 1 9.5 4.5a6 6 0 0 0 10 10z"),
    HalfCircle("M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 0 0 0-17zM12 3.5v17"),
    Chip("M7.5 7.5h9v9h-9zM10 3.5v4M14 3.5v4M10 16.5v4M14 16.5v4M3.5 10h4M3.5 14h4M16.5 10h4M16.5 14h4"),
    Info("M12 20.5a8.5 8.5 0 1 0 0-17 8.5 8.5 0 0 0 0 17zM12 11v5.5M12 7.75v.25"),
    Battery("M3.5 8h14v8h-14zM20.5 11v2"),
    SignOut("M13.5 4.5h-8v15h8M10 12h10.5M17 8.5l3.5 3.5-3.5 3.5"),
    Key("M8 15.5a3.5 3.5 0 1 1 3.2-5h9.3v3h-2v2h-3v-2h-4.3A3.5 3.5 0 0 1 8 15.5z"),
    Branch("M6.5 7.75v8.5M17.5 9.75c0 2.9-2.6 4.35-6.2 4.72-1.9.2-3.3.9-4 2.03M6.5 7.75a2.25 2.25 0 1 0 0-4.5 2.25 2.25 0 0 0 0 4.5zM6.5 20.75a2.25 2.25 0 1 0 0-4.5 2.25 2.25 0 0 0 0 4.5zM17.5 9.75a2.25 2.25 0 1 0 0-4.5 2.25 2.25 0 0 0 0 4.5z"),
    Chat("M12 20c4.7 0 8.5-3.4 8.5-7.5S16.7 5 12 5 3.5 8.4 3.5 12.5c0 1.6.6 3.1 1.6 4.3L4.5 20l3.6-1c1.2.6 2.5 1 3.9 1z"),
    Gear("M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM19.4 13.5l1.3 1-1.8 3.1-1.6-.5a7.5 7.5 0 0 1-1.9 1.1l-.3 1.6h-3.6l-.3-1.6a7.5 7.5 0 0 1-1.9-1.1l-1.6.5-1.8-3.1 1.3-1a7.6 7.6 0 0 1 0-2.2l-1.3-1 1.8-3.1 1.6.5a7.5 7.5 0 0 1 1.9-1.1l.3-1.6h3.6l.3 1.6a7.5 7.5 0 0 1 1.9 1.1l1.6-.5 1.8 3.1-1.3 1a7.6 7.6 0 0 1 0 2.2z"),
    Terminal("M4 5h16v14H4zM7.5 9.5L10 12l-2.5 2.5M12 15h4"),
    ArrowUpRight("M7.5 16.5l9-9M9 7.5h7.5V15"),
    Globe("M12 20.5a8.5 8.5 0 1 0 0-17 8.5 8.5 0 0 0 0 17zM3.5 12h17M12 3.5c2.3 2.3 3.5 5.1 3.5 8.5S14.3 18.2 12 20.5C9.7 18.2 8.5 15.4 8.5 12S9.7 5.8 12 3.5z"),
    Stack("M4 8l8-4 8 4-8 4zM4 12l8 4 8-4M4 16l8 4 8-4"),
    Gauge("M4.5 16.5a8 8 0 1 1 15 0M12 13.5l3.5-4.5M12 15a1.5 1.5 0 1 0 0-3"),
    Queue("M4.5 6.5h15M4.5 11.5h15M4.5 16.5h8M16 15l3 2.5-3 2.5"),
    Steer("M5 5v6.5a3 3 0 0 0 3 3h10.5M15 11l3.5 3.5L15 18"),
    StopCircle("M12 20.5a8.5 8.5 0 1 0 0-17 8.5 8.5 0 0 0 0 17zM9.5 9.5h5v5h-5z"),
    File("M6.5 3.5h7l4 4v13h-11zM13.5 3.5v4h4");

    val path: Path by lazy { PathParser.createPathFromPathData(d) }
}

@Composable
fun GlyphIcon(glyph: Glyph, tint: Color, size: Dp = 18.dp, modifier: Modifier = Modifier, weight: Float = 1.6f) {
    val path = remember(glyph) { glyph.path.asComposePath() }
    Canvas(modifier.size(size)) {
        val k = this.size.minDimension / 24f
        scale(k, k, pivot = androidx.compose.ui.geometry.Offset.Zero) {
            if (glyph.fill) drawPath(path, tint, style = Fill)
            else drawPath(path, tint, style = Stroke(width = weight, cap = StrokeCap.Round, join = StrokeJoin.Round))
        }
    }
}

/** Glyph onto an android.graphics canvas (the transcript painter). */
fun drawGlyph(canvas: Canvas, glyph: Glyph, rect: RectF, color: Int, paint: Paint) {
    val k = minOf(rect.width(), rect.height()) / 24f
    val m = Matrix().apply {
        setScale(k, k)
        postTranslate(rect.centerX() - 12f * k, rect.centerY() - 12f * k)
    }
    val p = Path(glyph.path).apply { transform(m) }
    paint.reset()
    paint.isAntiAlias = true
    paint.color = color
    if (glyph.fill) {
        paint.style = Paint.Style.FILL
    } else {
        paint.style = Paint.Style.STROKE
        paint.strokeWidth = 1.6f * k
        paint.strokeCap = Paint.Cap.ROUND
        paint.strokeJoin = Paint.Join.ROUND
    }
    canvas.drawPath(p, paint)
}

/**
 * The desktop's SVG icons (tool-*, fileicon-*, brand-*-mark, tab-*), the
 * same files the iOS asset catalog holds. Rasterized once per size and
 * appearance. Tool/brand icons are templates (tinted); file icons keep their
 * colors and have `-dark` variants.
 */
object SvgIcons {
    private lateinit var app: Context
    private val svgs = HashMap<String, SVG?>()
    private val bitmaps = LruCache<String, Bitmap>(256)

    fun init(context: Context) {
        app = context.applicationContext
    }

    fun isTemplate(name: String) = !name.startsWith("fileicon-")

    @Synchronized
    private fun svg(name: String): SVG? = svgs.getOrPut(name) {
        runCatching { app.assets.open("icons/$name.svg").use { SVG.getFromInputStream(it) } }.getOrNull()
    }

    fun exists(name: String): Boolean = svg(name) != null

    /**
     * `name` at `px` square. Templates render black (alpha is the shape) and
     * are tinted at draw time; file icons pick their dark variant.
     */
    fun bitmap(name: String, px: Int, dark: Boolean): Bitmap? {
        val variant = if (dark && !isTemplate(name) && svg("$name-dark") != null) "$name-dark" else name
        val key = "$variant@$px"
        bitmaps.get(key)?.let { return it }
        val svg = synchronized(this) { svg(variant) } ?: return null
        val size = px.coerceAtLeast(1)
        val bmp = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888)
        synchronized(svg) {
            svg.setDocumentWidth(size.toFloat())
            svg.setDocumentHeight(size.toFloat())
            svg.renderToCanvas(Canvas(bmp))
        }
        bitmaps.put(key, bmp)
        return bmp
    }

    private val tintPaint = Paint(Paint.FILTER_BITMAP_FLAG or Paint.ANTI_ALIAS_FLAG)

    /** Draw into `rect` (template icons tinted with `tint`). */
    fun draw(canvas: Canvas, name: String, rect: RectF, tint: Int, dark: Boolean, density: Float) {
        val bmp = bitmap(name, (rect.width() * density).toInt(), dark) ?: return
        tintPaint.colorFilter = if (isTemplate(name)) PorterDuffColorFilter(tint, PorterDuff.Mode.SRC_IN) else null
        canvas.drawBitmap(bmp, null, rect, tintPaint)
    }
}

@Composable
fun SvgIcon(name: String, size: Dp, tint: Color?, modifier: Modifier = Modifier) {
    val colors = LocalColors.current
    val px = with(LocalDensity.current) { size.roundToPx() }
    val bmp = remember(name, px, colors.dark) { SvgIcons.bitmap(name, px, colors.dark)?.asImageBitmap() } ?: return
    Image(
        bitmap = bmp,
        contentDescription = null,
        modifier = modifier.size(size),
        colorFilter = if (tint != null && SvgIcons.isTemplate(name)) ColorFilter.tint(tint) else null,
    )
}

/** Harness brand marks (desktop `*-mark.svg`). Claude keeps its orange. */
object BrandMarks {
    fun asset(harness: String?): String = "brand-" + when (harness) {
        "codex" -> "openai"
        "cursor" -> "cursor"
        "devin" -> "devin"
        "grok" -> "grok"
        "hermes" -> "hermes"
        "pi" -> "pi"
        "opencode" -> "opencode"
        "antigravity" -> "antigravity"
        else -> "claude"
    } + "-mark"
}

@Composable
fun BrandMark(harness: String?, size: Dp, modifier: Modifier = Modifier, tint: Color? = null) {
    val colors = LocalColors.current
    SvgIcon(BrandMarks.asset(harness), size, tint ?: colors.brand(harness) ?: colors.text, modifier)
}

fun Color.argb(): Int = toArgb()
