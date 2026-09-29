package sh.zeron.android.transcript

import android.graphics.Bitmap
import android.graphics.BitmapShader
import android.graphics.Canvas
import android.graphics.LinearGradient
import android.graphics.Matrix
import android.graphics.Paint
import android.graphics.Path
import android.graphics.PorterDuff
import android.graphics.PorterDuffXfermode
import android.graphics.RectF
import android.graphics.Shader
import android.os.SystemClock
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import sh.zeron.android.core.Formatters
import sh.zeron.android.design.Fonts
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.GlyphMotion
import sh.zeron.android.design.SvgIcons
import sh.zeron.android.design.drawGlyph
import uniffi.zeron_core.ColorRole
import uniffi.zeron_core.FaceRole
import uniffi.zeron_core.RowPlacement
import uniffi.zeron_core.Widget
import uniffi.zeron_core.WidgetKind
import kotlin.math.max
import kotlin.math.min

/**
 * Paints the native affordances a display list names but doesn't draw —
 * icons, images, spinners, chevrons, the activity rail, the working trailer,
 * the shimmer — at the Rust-given rects (layout units), and hit-tests the
 * interactive ones. Stateless per row except small animation memories.
 */
class WidgetPainter(private val view: TranscriptCanvas) {
    private val density = view.resources.displayMetrics.density
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.STROKE }
    private val rect = RectF()
    private val path = Path()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    /** Groups on screen when the transcript attaches never animate. */
    var quietUntil = 0L

    /** Loaded attachment images by reference. */
    val images = HashMap<String, Bitmap>()
    private val loading = HashSet<String>()
    private val failed = HashSet<String>()

    /** Copy buttons showing their checkmark (key, widget id) → until. */
    private val copied = HashMap<Pair<ULong, UInt>, Long>()

    /** Chevron rotation memory per group: last painted state + flip time. */
    private val chevrons = HashMap<ULong, Pair<Boolean, Long>>()

    var uploadProgress: Double? = null

    private val labelPaint by lazy { Fonts.paint(FaceRole.SANS_MEDIUM, 13.5f, true) }

    fun markCopied(key: ULong, id: UInt) {
        copied[key to id] = SystemClock.uptimeMillis() + 1400
    }

    /** Draw a row's widgets; true while something animates. */
    fun draw(canvas: Canvas, model: RowPaint, p: RowPlacement, offsets: FloatArray, now: Long): Boolean {
        val colors = view.colors
        var animating = false
        val d = model.display
        for (w in d.widgets) {
            val sc = w.scroller?.toInt()
            if (sc != null) {
                val s = d.scrollers[sc]
                canvas.save()
                canvas.clipRect(s.x, s.y, s.x + s.w, s.y + s.h)
                canvas.translate(s.x - offsets[sc], s.y)
            }
            rect.set(w.x, w.y, w.x + w.w, w.y + w.h)
            when (val k = w.kind) {
                WidgetKind.CopyCode -> {
                    val until = copied[model.key to w.id]
                    val done = until != null && until > now
                    if (until != null && !done) copied.remove(model.key to w.id)
                    if (done) animating = true
                    val s = min(rect.width(), rect.height()).coerceAtMost(28f) * 0.62f
                    val r = RectF(rect.centerX() - s / 2, rect.centerY() - s / 2, rect.centerX() + s / 2, rect.centerY() + s / 2)
                    drawGlyph(canvas, if (done) Glyph.Check else Glyph.Copy, r, if (done) colors.role(ColorRole.ACCENT) else colors.role(ColorRole.TEXT_TERTIARY), paint)
                }
                is WidgetKind.Disclosure, is WidgetKind.Detail, is WidgetKind.ToolToggle -> Unit // tap targets only
                is WidgetKind.ToolStatus -> {
                    if (k.running) {
                        animating = true
                        spinner(canvas, rect, now, colors.dark)
                    } else {
                        val s = min(rect.width(), rect.height()) * 0.8f
                        val r = RectF(rect.centerX() - s / 2, rect.centerY() - s / 2, rect.centerX() + s / 2, rect.centerY() + s / 2)
                        drawGlyph(canvas, if (k.failed) Glyph.Close else Glyph.Check, r, colors.role(if (k.failed) ColorRole.DANGER else ColorRole.TEXT_TERTIARY), paint)
                    }
                }
                is WidgetKind.Image -> image(canvas, k.reference)
                WidgetKind.Spinner -> {
                    animating = true
                    spinner(canvas, rect, now, colors.dark)
                }
                is WidgetKind.Working -> {
                    animating = true
                    working(canvas, k.sinceMs, k.streaming, now)
                }
                is WidgetKind.Icon -> icon(canvas, k.name, colors.role(k.color))
                is WidgetKind.Chevron -> {
                    val was = chevrons[model.key]?.first
                    if (was == null) chevrons[model.key] = k.expanded to 0L
                    else if (was != k.expanded) chevrons[model.key] = k.expanded to now
                    val flipAt = chevrons[model.key]!!.second
                    val t = if (flipAt == 0L) 1f else ((now - flipAt) / 140f).coerceIn(0f, 1f)
                    if (t < 1f) animating = true
                    val target = if (k.expanded) 0f else -90f
                    val from = if (k.expanded) -90f else 0f
                    val angle = from + (target - from) * (1 - (1 - t) * (1 - t))
                    canvas.save()
                    canvas.rotate(angle, rect.centerX(), rect.centerY())
                    SvgIcons.draw(canvas, "tool-alt-arrow-down", rect, colors.role(ColorRole.TEXT_SECONDARY), colors.dark, density)
                    canvas.restore()
                }
                is WidgetKind.ToolRail -> rail(canvas, k, colors.role(ColorRole.TOOL_RAIL))
                WidgetKind.Shimmer -> {
                    animating = true
                    shimmer(canvas, model, now)
                }
            }
            if (sc != null) canvas.restore()
        }
        return animating
    }

    /** Topmost interactive widget under a row-space point. */
    fun hit(model: RowPaint, x: Float, y: Float, offsets: FloatArray): Widget? {
        for (w in model.display.widgets.asReversed()) {
            val interactive = when (w.kind) {
                is WidgetKind.Disclosure, is WidgetKind.ToolToggle, WidgetKind.CopyCode, is WidgetKind.Detail, is WidgetKind.Image, is WidgetKind.Chevron -> true
                else -> false
            }
            if (!interactive) continue
            var lx = x
            var ly = y
            w.scroller?.let { si ->
                val s = model.display.scrollers[si.toInt()]
                lx = x - s.x + offsets[si.toInt()]
                ly = y - s.y
            }
            val pad = if (w.kind == WidgetKind.CopyCode) 8f else 0f
            if (lx >= w.x - pad && lx <= w.x + w.w + pad && ly >= w.y - pad && ly <= w.y + w.h + pad) return w
        }
        return null
    }

    // ── pieces ───────────────────────────────────────────────────────────

    private fun spinner(canvas: Canvas, r: RectF, now: Long, dark: Boolean) {
        val tints = if (dark) intArrayOf(0xFFABA1F9.toInt(), 0xFF8B7CF6.toInt(), 0xFF7266CA.toInt())
        else intArrayOf(0xFF7965EC.toInt(), 0xFF5B43E8.toInt(), 0xFF4332AC.toInt())
        val phase = (now % GlyphMotion.PERIOD_MS) / GlyphMotion.PERIOD_MS.toDouble()
        val cell = 3.5f * min(1f, min(r.width(), r.height()) / 12f)
        val gap = cell * 1.5f / 3.5f
        val ox = r.centerX() - (cell * 2 + gap) / 2
        val oy = r.centerY() - (cell * 3 + gap * 2) / 2
        paint.style = Paint.Style.FILL
        for (i in 0 until 6) {
            val row = i / 2
            val col = i % 2
            val a = GlyphMotion.opacity(phase - GlyphMotion.spinnerPhase(row, col))
            paint.color = tints[row]
            paint.alpha = (255 * a).toInt()
            canvas.drawCircle(ox + col * (cell + gap) + cell / 2, oy + row * (cell + gap) + cell / 2, cell / 2, paint)
        }
    }

    private val trailerTints = intArrayOf(0xFFB6D3EF.toInt(), 0xFFEDB185.toInt(), 0xFFF888A0.toInt())

    /** Tail-of-turn trailer: the 3×3 gradient spinner + "Working… 12s". */
    private fun working(canvas: Canvas, sinceMs: Long?, streaming: Boolean, now: Long) {
        val colors = view.colors
        val phase = (now % GlyphMotion.PERIOD_MS) / GlyphMotion.PERIOD_MS.toDouble()
        val cell = 3.5f
        val gap = 1.75f
        val side = cell * 3 + gap * 2
        val cx = rect.left + 7f
        val cy = rect.centerY()
        paint.style = Paint.Style.FILL
        for (i in 0 until 9) {
            val row = i / 3
            val col = i % 3
            val a = GlyphMotion.opacity(phase - GlyphMotion.trailerPhase(row, col))
            paint.color = trailerTints[row]
            paint.alpha = (255 * a).toInt()
            canvas.drawCircle(cx - side / 2 + col * (cell + gap) + cell / 2, cy - side / 2 + row * (cell + gap) + cell / 2, cell / 2, paint)
        }
        val word = if (streaming) "Writing…" else "Working…"
        val lp = labelPaint
        val fm = lp.fontMetrics
        val baseline = cy - (fm.ascent + fm.descent) / 2
        lp.color = colors.role(ColorRole.TEXT_SECONDARY)
        val x = rect.left + 22f
        canvas.drawText(word, x, baseline, lp)
        val secs = sinceMs?.let { max(0L, (System.currentTimeMillis() - it) / 1000) } ?: 0L
        if (secs > 0) {
            lp.color = colors.role(ColorRole.TEXT_TERTIARY)
            canvas.drawText("  " + Formatters.elapsed(secs), x + lp.measureText(word), baseline, lp)
        }
    }

    private fun icon(canvas: Canvas, name: String, color: Int) {
        val colors = view.colors
        when (name) {
            "checkmark.square.fill" -> {
                paint.style = Paint.Style.FILL
                paint.color = color
                val inset = rect.width() * 0.1f
                canvas.drawRoundRect(rect.left + inset, rect.top + inset, rect.right - inset, rect.bottom - inset, 3.5f, 3.5f, paint)
                val s = rect.width() * 0.7f
                drawGlyph(canvas, Glyph.Check, RectF(rect.centerX() - s / 2, rect.centerY() - s / 2, rect.centerX() + s / 2, rect.centerY() + s / 2), colors.backgroundArgb, stroke)
            }
            "square" -> drawGlyph(canvas, Glyph.Square, rect, color, paint)
            "exclamationmark.triangle" -> drawGlyph(canvas, Glyph.Warning, rect, color, paint)
            "questionmark.bubble" -> drawGlyph(canvas, Glyph.Question, rect, color, paint)
            "arrow.triangle.branch" -> drawGlyph(canvas, Glyph.Branch, rect, color, paint)
            else -> if (SvgIcons.exists(name)) {
                SvgIcons.draw(canvas, name, rect, color, colors.dark, density)
            } else {
                drawGlyph(canvas, Glyph.Info, rect, color, paint)
            }
        }
    }

    private val imagePaint = Paint(Paint.ANTI_ALIAS_FLAG or Paint.FILTER_BITMAP_FLAG)
    private val matrix = Matrix()

    private fun image(canvas: Canvas, reference: String) {
        val colors = view.colors
        val radius = if (min(rect.width(), rect.height()) > 120) 14f else 12f
        val bmp = images[reference]
        if (bmp == null) {
            paint.style = Paint.Style.FILL
            paint.color = colors.role(ColorRole.CHIP_BACKGROUND)
            canvas.drawRoundRect(rect, radius, radius, paint)
            load(reference)
        } else {
            // Aspect-fill into the rounded rect.
            val scale = max(rect.width() / bmp.width, rect.height() / bmp.height)
            matrix.setScale(scale, scale)
            matrix.postTranslate(rect.centerX() - bmp.width * scale / 2, rect.centerY() - bmp.height * scale / 2)
            val shader = BitmapShader(bmp, Shader.TileMode.CLAMP, Shader.TileMode.CLAMP)
            shader.setLocalMatrix(matrix)
            imagePaint.shader = shader
            canvas.drawRoundRect(rect, radius, radius, imagePaint)
            imagePaint.shader = null
        }
        if (reference.startsWith("pending://")) uploadRing(canvas, radius)
    }

    /** Upload progress over a pending thumbnail: scrim + ring + percentage. */
    private fun uploadRing(canvas: Canvas, radius: Float) {
        val p = uploadProgress ?: return
        paint.style = Paint.Style.FILL
        paint.color = 0x61000000
        canvas.drawRoundRect(rect, radius, radius, paint)
        val dia = min(34f, min(rect.width(), rect.height()) * 0.55f)
        val r = RectF(rect.centerX() - dia / 2, rect.centerY() - dia / 2, rect.centerX() + dia / 2, rect.centerY() + dia / 2)
        stroke.strokeWidth = 3f
        stroke.strokeCap = Paint.Cap.ROUND
        stroke.color = 0x4DFFFFFF
        canvas.drawOval(r, stroke)
        stroke.color = 0xFFFFFFFF.toInt()
        canvas.drawArc(r, -90f, 360f * p.toFloat().coerceIn(0.02f, 1f), false, stroke)
    }

    private fun load(reference: String) {
        if (reference in loading || reference in failed) return
        loading.add(reference)
        scope.launch {
            val bmp = runCatching { view.host?.loadImage(reference) }.getOrNull()
            loading.remove(reference)
            if (bmp != null) images[reference] = bmp else failed.add(reference)
            view.invalidate()
        }
    }

    /** The activity rail: a 1 dp trunk with a quadratic elbow into each row. */
    private fun rail(canvas: Canvas, k: WidgetKind.ToolRail, color: Int) {
        stroke.color = color
        stroke.strokeWidth = 1f
        stroke.strokeCap = Paint.Cap.BUTT
        canvas.save()
        canvas.translate(rect.left, rect.top)
        path.reset()
        for (i in k.tops.indices) {
            val top = k.tops[i]
            val mid = top + k.rowMid
            path.moveTo(k.trunkX, top)
            path.lineTo(k.trunkX, mid - k.bend)
            path.quadTo(k.trunkX, mid, k.trunkX + k.bend, mid)
            path.lineTo(k.branchEnd, mid)
            if (i + 1 < k.tops.size) {
                path.moveTo(k.trunkX, mid - k.bend)
                path.lineTo(k.trunkX, top + k.heights[i])
            }
        }
        canvas.drawPath(path, stroke)
        canvas.restore()
    }

    private val sweep = Paint().apply { xfermode = PorterDuffXfermode(PorterDuff.Mode.DST_IN) }

    /**
     * Shimmer over the live group's title: the same runs re-drawn in `text`
     * through a moving triangle mask (3.4 s sweep, highlights every 3 widths).
     */
    private fun shimmer(canvas: Canvas, model: RowPaint, now: Long) {
        val colors = view.colors
        val w = max(rect.width(), 1f)
        val saved = canvas.saveLayer(rect, null)
        model.drawRunsIn(canvas, rect.left, rect.top, rect.right, rect.bottom, colors.role(ColorRole.TEXT), colors)
        val t = (now % 3400) / 3400f
        val half = 0.36f / 9
        val stops = FloatArray(9)
        val cs = IntArray(9)
        for (kk in 0 until 3) {
            val c = (1.5f + 3 * kk) / 9
            stops[kk * 3] = c - half; cs[kk * 3] = 0
            stops[kk * 3 + 1] = c; cs[kk * 3 + 1] = 0xFF000000.toInt()
            stops[kk * 3 + 2] = c + half; cs[kk * 3 + 2] = 0
        }
        // The 9-width mask slides from -6w to 0 (highlights cross the title).
        val x0 = rect.left - 6 * w + t * 6 * w
        sweep.shader = LinearGradient(x0, 0f, x0 + 9 * w, 0f, cs, stops, Shader.TileMode.CLAMP)
        canvas.drawRect(rect, sweep)
        canvas.restoreToCount(saved)
    }

    fun close() = scope.cancel()
}
