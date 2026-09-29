package sh.zeron.android.transcript

import android.graphics.Canvas
import android.graphics.LinearGradient
import android.graphics.Paint
import android.graphics.PorterDuff
import android.graphics.PorterDuffXfermode
import android.graphics.RecordingCanvas
import android.graphics.RenderNode
import android.graphics.Shader
import sh.zeron.android.design.Fonts
import sh.zeron.android.design.ZeronColors
import uniffi.zeron_core.BoxStyle
import uniffi.zeron_core.Decoration
import uniffi.zeron_core.FadeEdge
import uniffi.zeron_core.RowDisplay
import uniffi.zeron_core.StyleDesc
import uniffi.zeron_core.TextRun
import java.util.concurrent.ConcurrentHashMap

/**
 * style id → Paint for one transcript (ids are per layout view). Paints use
 * the exact face bytes Rust measured; sizes are in layout units (dp) — the
 * row canvases are scaled by density, so glyphs rasterize at device size.
 */
class StylePaints {
    private val paints = ConcurrentHashMap<Int, Paint>()
    @Volatile var count = 0
        private set

    fun update(styles: List<StyleDesc>) {
        for (s in styles) {
            val id = s.id.toInt()
            if (!paints.containsKey(id)) paints[id] = Fonts.paint(s.face, s.size, s.ligatures)
        }
        count = paints.size
    }

    operator fun get(id: Int): Paint? = paints[id]
}

/**
 * A display list plus everything derived from it once per (row, version,
 * width): runs/boxes grouped by layer (0 = the row, n = scroller n-1),
 * overflow-fade membership, and lazily recorded [RenderNode]s so a row's
 * static content is replayed — not re-issued — on every scroll frame.
 * Built off the main thread (prefetch); nodes record on first draw.
 */
class RowPaint(val display: RowDisplay, private val paints: StylePaints) {
    val key: ULong get() = display.key
    val version: ULong get() = display.version
    val width: Float get() = display.width
    private val layers = display.scrollers.size + 1
    val runsByLayer: Array<IntArray>
    private val boxesByLayer: Array<IntArray>
    /** Per layer: (fade index, runs it masks); masked runs skip the plain pass. */
    private val fadesByLayer: Array<List<Pair<Int, IntArray>>>
    private val faded: BooleanArray
    /** Animated widgets (spinners, working, shimmer) force per-frame redraws. */
    val animated: Boolean

    init {
        val runs = Array(layers) { ArrayList<Int>() }
        display.runs.forEachIndexed { i, r -> runs[layer(r.scroller)].add(i) }
        val boxes = Array(layers) { ArrayList<Int>() }
        display.boxes.forEachIndexed { i, b -> boxes[layer(b.scroller)].add(i) }
        val fadedRuns = BooleanArray(display.runs.size)
        val fades = Array(layers) { ArrayList<Pair<Int, IntArray>>() }
        display.fades.forEachIndexed { fi, f ->
            val l = layer(f.scroller)
            val masked = runs[l].filter { i ->
                val r = display.runs[i]
                !fadedRuns[i] && r.baseline > f.y && r.baseline <= f.y + f.h + 0.5f && (f.edge == FadeEdge.BOTTOM || r.x < f.x + f.w)
            }
            masked.forEach { fadedRuns[it] = true }
            fades[l].add(fi to masked.toIntArray())
        }
        runsByLayer = Array(layers) { runs[it].toIntArray() }
        boxesByLayer = Array(layers) { boxes[it].toIntArray() }
        fadesByLayer = Array(layers) { fades[it] }
        faded = fadedRuns
        animated = display.widgets.any {
            when (val k = it.kind) {
                is uniffi.zeron_core.WidgetKind.Working, uniffi.zeron_core.WidgetKind.Spinner, uniffi.zeron_core.WidgetKind.Shimmer -> true
                is uniffi.zeron_core.WidgetKind.ToolStatus -> k.running
                else -> false
            }
        }
    }

    private fun layer(scroller: UInt?) = scroller?.let { it.toInt() + 1 } ?: 0

    // ── recorded nodes ───────────────────────────────────────────────────

    private var nodes = arrayOfNulls<RenderNode>(layers)
    private var nodesDark: Boolean? = null
    private var nodeDensity = 0f
    /**
     * The veil each layer was recorded with. Per layer: only the row layer
     * veils, and re-recording one layer must never wipe another that the
     * current frame already drew (it would paint empty).
     */
    private val nodeVeils = IntArray(layers) { -1 }

    /**
     * The recorded static content of `layer` (boxes, runs, fades) in layout
     * units, scaled to pixels. `veilFrom` ≥ 0 records only settled text
     * (the fresh tail is drawn live while it fades in).
     */
    fun node(layer: Int, colors: ZeronColors, density: Float, veilFrom: Int): RenderNode {
        if (nodesDark != colors.dark || nodeDensity != density) {
            // Theme/density changes happen between frames: drop everything.
            nodes.forEach { it?.discardDisplayList() }
            nodes = arrayOfNulls(layers)
            nodesDark = colors.dark
            nodeDensity = density
        }
        // HWUI deletes a node's display list once it leaves the drawn tree
        // (a row scrolled off screen): re-record then, reuse otherwise.
        nodes[layer]?.let { if (it.hasDisplayList() && nodeVeils[layer] == veilFrom) return it }
        nodeVeils[layer] = veilFrom
        val w = if (layer == 0) display.width else display.scrollers[layer - 1].contentWidth
        val h = if (layer == 0) display.height else display.scrollers[layer - 1].h
        // Re-recording an existing node replaces its content in place.
        val node = nodes[layer] ?: RenderNode("row").apply {
            setPosition(0, 0, (w * density).toInt() + 2, (h * density).toInt() + 2)
            setClipToBounds(false)
        }
        val canvas: RecordingCanvas = node.beginRecording()
        try {
            canvas.scale(density, density)
            paintLayer(canvas, layer, colors, 1f / density, if (veilFrom >= 0) Pass.Settled(veilFrom) else Pass.All)
        } finally {
            node.endRecording()
        }
        nodes[layer] = node
        return node
    }

    fun release() {
        nodes.forEach { it?.discardDisplayList() }
        nodes = arrayOfNulls(layers)
        nodesDark = null
        nodeVeils.fill(-1)
    }

    sealed interface Pass {
        data object All : Pass
        data class Settled(val veilFrom: Int) : Pass
        data class Fresh(val veilFrom: Int) : Pass
    }

    private val boxPaint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val decoPaint = Paint()

    /** Paint one layer in its own coordinates (layout units). */
    fun paintLayer(canvas: Canvas, layer: Int, colors: ZeronColors, hairline: Float, pass: Pass, alpha: Float = 1f) {
        if (pass !is Pass.Fresh) {
            for (i in boxesByLayer[layer]) {
                val b = display.boxes[i]
                boxPaint.color = colors.role(b.color)
                when (b.style) {
                    BoxStyle.FILL -> {
                        boxPaint.style = Paint.Style.FILL
                        canvas.drawRoundRect(b.x, b.y, b.x + b.w, b.y + b.h, b.radius, b.radius, boxPaint)
                    }
                    BoxStyle.HAIRLINE -> {
                        boxPaint.style = Paint.Style.STROKE
                        boxPaint.strokeWidth = hairline
                        val r = maxOf(0f, b.radius - hairline / 2)
                        val o = hairline / 2
                        canvas.drawRoundRect(b.x + o, b.y + o, b.x + b.w - o, b.y + b.h - o, r, r, boxPaint)
                    }
                }
            }
        }
        for (i in runsByLayer[layer]) {
            if (faded[i]) continue
            val run = display.runs[i]
            val start = run.start.toInt()
            val end = start + run.len.toInt()
            when (pass) {
                Pass.All -> drawRun(canvas, run, colors, hairline, alpha = alpha)
                is Pass.Settled -> {
                    if (start >= pass.veilFrom) continue
                    if (end > pass.veilFrom) {
                        // Split a run straddling the veil at the glyph edge.
                        val dx = advance(run, pass.veilFrom)
                        canvas.save()
                        canvas.clipRect(run.x, -10_000f, run.x + dx, 10_000f)
                        drawRun(canvas, run, colors, hairline)
                        canvas.restore()
                    } else {
                        drawRun(canvas, run, colors, hairline)
                    }
                }
                is Pass.Fresh -> {
                    if (end <= pass.veilFrom) continue
                    if (start < pass.veilFrom) {
                        val dx = advance(run, pass.veilFrom)
                        canvas.save()
                        canvas.clipRect(run.x + dx, -10_000f, 10_000f, 10_000f)
                        drawRun(canvas, run, colors, hairline, alpha = alpha)
                        canvas.restore()
                    } else {
                        drawRun(canvas, run, colors, hairline, alpha = alpha)
                    }
                }
            }
        }
        if (pass !is Pass.Fresh) drawFades(canvas, layer, colors, hairline)
    }

    /** Width of `run`'s text up to UTF-16 offset `until` (veil split). */
    private fun advance(run: TextRun, until: Int): Float {
        val p = paints[run.style.toInt()] ?: return 0f
        val start = run.start.toInt()
        return p.measureText(display.text, start, until.coerceIn(start, start + run.len.toInt()))
    }

    fun drawRun(canvas: Canvas, run: TextRun, colors: ZeronColors, hairline: Float, colorOverride: Int? = null, alpha: Float = 1f) {
        val p = paints[run.style.toInt()] ?: return
        val start = run.start.toInt()
        val end = start + run.len.toInt()
        if (end > display.text.length) return
        val base = colorOverride ?: colors.role(run.color)
        val color = if (alpha >= 1f) base else (((base ushr 24) * alpha).toInt() shl 24) or (base and 0xFFFFFF)
        p.color = color
        canvas.drawText(display.text, start, end, run.x, run.baseline, p)
        if (run.decoration != Decoration.NONE) {
            decoPaint.color = color
            val y = if (run.decoration == Decoration.UNDERLINE) run.baseline + 2f else run.baseline - 5f
            canvas.drawRect(run.x, y, run.x + run.width, y + maxOf(hairline, 1f), decoPaint)
        }
    }

    private val erase = Paint().apply { xfermode = PorterDuffXfermode(PorterDuff.Mode.DST_OUT) }

    /**
     * Runs under a fade: painted into a layer, clipped at a trailing fade's
     * end, then erased along an alpha ramp — the text itself fades, over any
     * background.
     */
    private fun drawFades(canvas: Canvas, layer: Int, colors: ZeronColors, hairline: Float) {
        for ((fi, runs) in fadesByLayer[layer]) {
            if (runs.isEmpty()) continue
            val f = display.fades[fi]
            val trailing = f.edge == FadeEdge.TRAILING
            val left = if (trailing) -100_000f else f.x - 40f
            val right = if (trailing) f.x + f.w else f.x + f.w + 40f
            val top = if (trailing) f.y - 40f else f.y - 400f
            val bottom = if (trailing) f.y + f.h + 40f else f.y + f.h + 40f
            val saved = canvas.saveLayer(maxOf(left, -2000f), top, right, bottom, null)
            canvas.clipRect(left, top, right, bottom)
            for (i in runs) drawRun(canvas, display.runs[i], colors, hairline)
            erase.shader = if (trailing) {
                LinearGradient(f.x, 0f, f.x + f.w, 0f, 0x00000000, 0xFF000000.toInt(), Shader.TileMode.CLAMP)
            } else {
                LinearGradient(0f, f.y, 0f, f.y + f.h, 0x00000000, 0xEB000000.toInt(), Shader.TileMode.CLAMP)
            }
            if (trailing) canvas.drawRect(f.x, top, f.x + f.w + 1f, bottom, erase)
            else canvas.drawRect(f.x - 40f, f.y, f.x + f.w + 40f, f.y + f.h + 40f, erase)
            canvas.restoreToCount(saved)
        }
    }

    /** Canvas-layer runs whose baseline sits inside the rect (shimmer re-draw). */
    fun drawRunsIn(canvas: Canvas, x0: Float, y0: Float, x1: Float, y1: Float, color: Int, colors: ZeronColors) {
        for (i in runsByLayer[0]) {
            val r = display.runs[i]
            if (r.baseline <= y0 || r.baseline > y1 + 2 || r.x >= x1) continue
            drawRun(canvas, r, colors, 1f, colorOverride = color)
        }
    }
}
