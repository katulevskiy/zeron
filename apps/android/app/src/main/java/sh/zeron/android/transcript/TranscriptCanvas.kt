package sh.zeron.android.transcript

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.Choreographer
import android.view.MotionEvent
import android.view.VelocityTracker
import android.view.View
import android.view.ViewConfiguration
import android.widget.EdgeEffect
import android.widget.OverScroller
import sh.zeron.android.design.ZeronColors
import uniffi.zeron_core.LayoutFrame
import uniffi.zeron_core.LayoutListener
import uniffi.zeron_core.RowKind
import uniffi.zeron_core.RowPlacement
import uniffi.zeron_core.TranscriptView
import uniffi.zeron_core.Widget
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.abs
import kotlin.math.exp
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow

/**
 * Virtualized transcript host. Rust owns geometry: each [LayoutFrame] gives
 * exact row heights and prefix-sum offsets, so this view only (1) asks which
 * rows intersect the viewport, (2) replays their recorded display lists at
 * those offsets, and (3) keeps the user's place across frames (anchor, or
 * follow the tail). No row is ever measured here.
 *
 * Units: everything is in layout units (dp) — `density` scales at draw time.
 */
@SuppressLint("ViewConstructor")
class TranscriptCanvas(
    context: Context,
    val engine: TranscriptView,
) : View(context) {

    interface Host {
        fun toggle(key: ULong)
        fun toggleDetail(row: ULong, detail: ULong, open: Boolean)
        fun openLink(url: String)
        fun openImage(reference: String, bitmap: Bitmap?)
        fun showDetail(title: String, text: String)
        fun copied(text: String)
        fun longPress(index: UInt, copyText: String, messageText: String?)
        fun tapEmpty()
        fun followChanged(following: Boolean, distanceFromBottom: Float)
        fun frameShown(rows: Int)
        suspend fun loadImage(reference: String): Bitmap?
    }

    var host: Host? = null
    var colors: ZeronColors = ZeronColors.Dark
        set(v) {
            if (field.dark != v.dark) {
                field = v
                invalidate()
            } else {
                field = v
            }
        }

    private val density = resources.displayMetrics.density
    private val paints = StylePaints()
    val widgets = WidgetPainter(this)

    // ── frames ───────────────────────────────────────────────────────────

    var current: LayoutFrame? = null
        private set
    private var firstFrame: LayoutFrame? = null
    private val framePending = AtomicBoolean(false)
    private val main = Handler(Looper.getMainLooper())

    /** Frame-ready pings from the layout thread, coalesced to one apply per turn. */
    val listener = object : LayoutListener {
        override fun frameReady(revision: ULong) {
            if (closed) return
            if (framePending.compareAndSet(false, true)) {
                main.post {
                    framePending.set(false)
                    // The screen may have gone (and freed the view) since the ping.
                    if (!closed) apply(engine.frame())
                }
            }
        }
    }

    /** Set by [close]: the Rust view is about to be freed; nothing may touch it. */
    @Volatile private var closed = false

    // ── scroll state (layout units) ──────────────────────────────────────

    private var scroll = 0f
    private var contentHeight = 0f
    var topInset = 0f
        set(v) {
            if (field != v) {
                field = v
                insetsChanged()
            }
        }
    var bottomInset = 0f
        set(v) {
            if (field != v) {
                field = v
                insetsChanged()
            }
        }
    private val viewH get() = height / density
    private val maxScroll get() = max(-topInset, contentHeight + bottomInset - viewH)
    private val minScroll get() = -topInset
    val distanceFromBottom get() = maxScroll - scroll

    /** Following the tail: new content keeps the bottom in view. */
    var following = true
        private set

    private fun setFollowing(v: Boolean) {
        if (following == v) return
        following = v
        if (!v) stopSpring()
        report()
    }

    private var lastReported = Float.NaN
    private fun report() {
        val d = distanceFromBottom
        if (abs(d - lastReported) > 4f || d <= 0f) {
            lastReported = d
            host?.followChanged(following, d)
        }
    }

    // ── row models ───────────────────────────────────────────────────────

    private data class ModelKey(val key: Long, val version: Long, val width: Float)

    private val cache = object : LinkedHashMap<ModelKey, RowPaint>(64, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<ModelKey, RowPaint>): Boolean {
            val evict = size > 400
            if (evict && shown[eldest.key.key] !== eldest.value) eldest.value.release()
            return evict
        }
    }

    /** The model each on-screen row is painting now (kept while an upgrade builds). */
    private val shown = HashMap<Long, RowPaint>()
    private val inflight = HashSet<ModelKey>()
    private val builder = Executors.newSingleThreadExecutor { r -> Thread(r, "zeron-rows").apply { priority = Thread.NORM_PRIORITY - 1 } }

    /** Streaming veil: freshly appended text fades in over 220 ms. */
    private class Veil(val from: Int, val start: Long)
    private val veils = HashMap<Long, Veil>()
    /** Rows that appear after the first frame fade in (280 ms). */
    private val appeared = HashMap<Long, Long>()

    // ── visible band ─────────────────────────────────────────────────────

    private var band: List<RowPlacement> = emptyList()
    private var bandY0 = 0f
    private var bandY1 = -1f
    private val overscan = 700f

    private fun visibleBand(): List<RowPlacement> {
        val frame = current ?: return emptyList()
        val y0 = scroll
        val y1 = scroll + viewH
        if (y0 < bandY0 || y1 > bandY1) {
            bandY0 = y0 - overscan
            bandY1 = y1 + overscan
            band = frame.rowsIn(bandY0, bandY1)
            prefetch(frame)
        }
        return band
    }

    private fun invalidateBand() {
        bandY1 = -1f
        bandY0 = 0f
    }

    // ── viewport ─────────────────────────────────────────────────────────

    private var viewportWidth = 0f
    private var textScale = 1f

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        val scale = resources.configuration.fontScale.coerceIn(0.85f, 1.6f)
        val width = w / density
        if (width != viewportWidth || scale != textScale) {
            viewportWidth = width
            textScale = scale
            if (width > 0) engine.setViewport(width, scale)
        }
        if (following) scroll = maxScroll
        invalidateBand()
        report()
    }

    private fun insetsChanged() {
        current?.let { contentHeight = contentHeightFor(it) }
        if (following && !dragging) scroll = maxScroll
        invalidateBand()
        invalidate()
        report()
    }

    // ── applying frames ──────────────────────────────────────────────────

    fun apply(frame: LayoutFrame) {
        if (frame.styleCount().toInt() != paints.count) paints.update(frame.styles())
        val old = current
        val first = old == null || old.rowCount() == 0u
        var anchor: Pair<ULong, Float>? = null
        if (!following && old != null) {
            val y = max(0f, scroll + topInset)
            old.indexAt(y)?.let { i -> old.placement(i)?.let { p -> anchor = p.key to (scroll - p.y) } }
        }
        current = frame
        if (first && frame.rowCount() > 0u) {
            firstFrame = frame
            widgets.quietUntil = SystemClock.uptimeMillis() + 800
        }
        contentHeight = contentHeightFor(frame)
        invalidateBand()
        when {
            first -> scroll = maxScroll
            following -> if (!dragging) startSpring()
            else -> anchor?.let { (key, delta) ->
                frame.indexOf(key)?.let { i -> frame.placement(i) }?.let { p ->
                    val target = p.y + delta
                    if (abs(target - scroll) > 0.5f) scroll = target.coerceIn(minScroll, max(minScroll, maxScroll))
                }
            }
        }
        if (!following) scroll = scroll.coerceIn(minScroll, maxScroll)
        if (frame.rowCount() > 0u) host?.frameShown(frame.rowCount().toInt())
        invalidate()
        report()
    }

    // ── own-turn runway (desktop OwnTurnAnchor) ──────────────────────────
    //
    // On an immediate send the prompt glides to the top of the viewport and
    // the content is held at least one viewport tall below it, so the reply
    // streams into reserved space without the view moving on every token.

    private class OwnTurn(var key: ULong?, val before: Set<ULong>)

    private var ownTurn: OwnTurn? = null
    private var queuedTurn: Set<ULong>? = null
    private var runway: Float? = null

    private fun recentUserKeys(): Set<ULong> {
        val frame = current ?: return emptySet()
        val n = frame.rowCount().toInt()
        val out = HashSet<ULong>()
        for (i in n - 1 downTo max(0, n - 24)) frame.placement(i.toUInt())?.let { if (it.kind == RowKind.USER) out.add(it.key) }
        return out
    }

    /** Reserve the reply's space below the prompt about to be sent. */
    fun beginOwnTurn() {
        queuedTurn = null
        ownTurn = OwnTurn(null, recentUserKeys())
        setFollowing(true)
    }

    /** A message queued behind the live turn takes the runway once its bubble lands. */
    fun expectQueuedTurn() {
        if (queuedTurn == null) queuedTurn = recentUserKeys()
    }

    private fun newUserRow(frame: LayoutFrame, before: Set<ULong>): RowPlacement? {
        val n = frame.rowCount().toInt()
        for (i in n - 1 downTo max(0, n - 12)) {
            val p = frame.placement(i.toUInt()) ?: continue
            if (p.kind == RowKind.USER && p.key !in before) return p
        }
        return null
    }

    private fun contentHeightFor(frame: LayoutFrame): Float {
        val natural = frame.totalHeight()
        queuedTurn?.let { before ->
            newUserRow(frame, before)?.let { p ->
                queuedTurn = null
                ownTurn = OwnTurn(p.key, before)
                setFollowing(true)
            }
        }
        val turn = ownTurn ?: run {
            runway = null
            return natural
        }
        if (turn.key == null) turn.key = newUserRow(frame, turn.before)?.key
        val key = turn.key
        val p = key?.let { frame.indexOf(it) }?.let { frame.placement(it) }
            ?: return max(natural, runway ?: 0f) // echo not laid out yet: hold still
        val hold = p.y - topInset
        val minimum = hold + viewH - bottomInset
        if (natural >= minimum - 0.5f) {
            runway = null
            return natural
        }
        runway = minimum
        return minimum
    }

    // ── follow spring ────────────────────────────────────────────────────

    private var springing = false
    private var lastTick = 0L

    private val springTick = object : Choreographer.FrameCallback {
        override fun doFrame(frameTimeNanos: Long) {
            if (!springing) return
            val dt = min(1.0 / 30, max(0.0, (frameTimeNanos - lastTick) / 1e9))
            lastTick = frameTimeNanos
            if (!following || dragging) return stopSpring()
            val target = maxScroll
            val delta = target - scroll
            if (abs(delta) < 0.5f) {
                scroll = target
                invalidate()
                report()
                return stopSpring()
            }
            scroll += if (runway != null) {
                // Runway glide: the desktop's frame-rate-independent ease-out.
                val frames = min(8.0, dt / (1.0 / 60))
                delta * (1 - 0.85.pow(frames)).toFloat()
            } else {
                // Critically damped approach: the tail "settles" into place.
                delta * (1 - exp(-dt * 16)).toFloat()
            }
            invalidate()
            report()
            Choreographer.getInstance().postFrameCallback(this)
        }
    }

    private fun startSpring() {
        if (springing) return
        springing = true
        lastTick = System.nanoTime()
        Choreographer.getInstance().postFrameCallback(springTick)
    }

    private fun stopSpring() {
        if (!springing) return
        springing = false
        Choreographer.getInstance().removeFrameCallback(springTick)
    }

    /** Jump (or glide) to the newest content and resume following. */
    fun scrollToBottom(animated: Boolean) {
        fling.forceFinished(true)
        setFollowing(true)
        if (animated) startSpring() else {
            scroll = maxScroll
            invalidate()
        }
    }

    // ── drawing ──────────────────────────────────────────────────────────

    private var animating = false
    private val thumb = Paint(Paint.ANTI_ALIAS_FLAG)
    private var lastScrollMs = 0L

    override fun onDraw(canvas: Canvas) {
        if (closed) return
        val frame = current ?: return
        if (fling.computeScrollOffset()) {
            scroll = (fling.currY / density)
            onFlingStep()
        }
        val rows = visibleBand()
        val now = SystemClock.uptimeMillis()
        animating = false
        val y0 = scroll
        val y1 = scroll + viewH
        canvas.save()
        canvas.scale(density, density)
        canvas.translate(0f, -scroll)
        for (p in rows) {
            if (p.y + p.height < y0 || p.y > y1) continue
            val model = model(p, frame) ?: continue
            drawRow(canvas, p, model, now)
        }
        canvas.restore()
        drawEdges(canvas)
        drawThumb(canvas, now)
        if (animating || !fling.isFinished) postInvalidateOnAnimation()
    }

    private fun drawRow(canvas: Canvas, p: RowPlacement, model: RowPaint, now: Long) {
        val key = p.key.toLong()
        canvas.save()
        canvas.translate(0f, p.y)
        val appear = appeared[key]?.let { ((now - it) / 280f).coerceIn(0f, 1f) } ?: 1f
        if (appear < 1f) {
            animating = true
            canvas.saveLayerAlpha(0f, 0f, model.width, p.height, (255 * easeOut(appear)).toInt())
        } else if (appeared.containsKey(key)) {
            appeared.remove(key)
        }
        val veil = veils[key]?.takeIf { now - it.start < 220 }
        if (veils.containsKey(key) && veil == null) veils.remove(key)
        val hairline = 1f / density
        // Static content: the recorded node (or a direct paint without HW).
        if (canvas.isHardwareAccelerated) {
            val node = model.node(0, colors, density, veil?.from ?: -1)
            canvas.save()
            canvas.scale(1f / density, 1f / density)
            canvas.drawRenderNode(node)
            canvas.restore()
        } else {
            model.paintLayer(canvas, 0, colors, hairline, veil?.let { RowPaint.Pass.Settled(it.from) } ?: RowPaint.Pass.All)
        }
        if (veil != null) {
            animating = true
            val t = easeOut(((now - veil.start) / 220f).coerceIn(0f, 1f))
            model.paintLayer(canvas, 0, colors, hairline, RowPaint.Pass.Fresh(veil.from), alpha = t)
        }
        // Horizontal scrollers (code blocks, wide tables).
        val offsets = scrollerOffsets(key, model)
        model.display.scrollers.forEachIndexed { i, s ->
            canvas.save()
            canvas.clipRect(s.x, s.y, s.x + s.w, s.y + s.h)
            canvas.translate(s.x - offsets[i], s.y)
            if (canvas.isHardwareAccelerated) {
                val node = model.node(i + 1, colors, density, -1)
                canvas.save()
                canvas.scale(1f / density, 1f / density)
                canvas.drawRenderNode(node)
                canvas.restore()
            } else {
                model.paintLayer(canvas, i + 1, colors, hairline, RowPaint.Pass.All)
            }
            canvas.restore()
        }
        // Native affordances (icons, images, spinners, disclosure chevrons).
        if (widgets.draw(canvas, model, p, offsets, now)) animating = true
        if (appear < 1f) canvas.restore()
        canvas.restore()
    }

    private fun easeOut(t: Float) = 1f - (1f - t) * (1f - t)

    private fun drawThumb(canvas: Canvas, now: Long) {
        val total = contentHeight + topInset + bottomInset
        if (total <= viewH * 1.5f) return
        val age = now - lastScrollMs
        if (age > 1200) return
        val a = if (age < 800) 1f else 1f - (age - 800) / 400f
        animating = true
        val trackTop = topInset + 4
        val trackH = viewH - topInset - bottomInset - 8
        val h = max(36f, trackH * viewH / total)
        val frac = ((scroll - minScroll) / max(1f, maxScroll - minScroll)).coerceIn(0f, 1f)
        val y = trackTop + frac * (trackH - h)
        thumb.color = colors.role(uniffi.zeron_core.ColorRole.TEXT_TERTIARY)
        thumb.alpha = (140 * a).toInt()
        val x = width / density - 5f
        canvas.save()
        canvas.scale(density, density)
        canvas.drawRoundRect(x, y, x + 3f, y + h, 1.5f, 1.5f, thumb)
        canvas.restore()
    }

    // ── models ───────────────────────────────────────────────────────────

    private fun model(p: RowPlacement, frame: LayoutFrame): RowPaint? {
        val key = p.key.toLong()
        val k = ModelKey(key, p.version.toLong(), frame.width())
        cache[k]?.let { return adopt(key, it) }
        val showing = shown[key]
        if (showing != null && showing.width == frame.width()) {
            // Same row, new content (a streaming tail): keep painting the
            // current model and build the new one off the main thread.
            buildAsync(p, frame, k)
            return showing
        }
        val display = frame.display(p.index) ?: return null
        val m = RowPaint(display, paints)
        cache[k] = m
        if (firstFrame?.indexOf(p.key) == null && firstFrame !== frame && !appeared.containsKey(key) && showing == null) {
            appeared[key] = SystemClock.uptimeMillis()
        }
        return adopt(key, m)
    }

    /** Swap a row's painted model; a grown streaming row veils its new text. */
    private fun adopt(key: Long, m: RowPaint): RowPaint {
        val old = shown[key]
        if (old === m) return m
        if (old != null && old.width == m.width) {
            val a = old.display.text
            val b = m.display.text
            if (b.length > a.length && b.startsWith(a)) veils[key] = Veil(a.length, SystemClock.uptimeMillis())
        }
        shown[key] = m
        if (old != null && old !== m && !cache.containsValue(old)) old.release()
        return m
    }

    private fun buildAsync(p: RowPlacement, frame: LayoutFrame, k: ModelKey) {
        if (closed || !inflight.add(k)) return
        builder.execute {
            val display = runCatching { frame.display(p.index) }.getOrNull()
            val m = display?.let { RowPaint(it, paints) }
            main.post {
                inflight.remove(k)
                if (m != null && !closed) {
                    cache[k] = m
                    invalidate()
                }
            }
        }
    }

    /** Build display models for rows just beyond the band off-main. */
    private fun prefetch(frame: LayoutFrame) {
        if (closed) return
        val ahead = viewH * 1.5f
        val down = lastDirection >= 0
        val ranges = if (down) listOf(bandY1 to bandY1 + ahead, bandY0 - ahead / 2 to bandY0)
        else listOf(bandY0 - ahead to bandY0, bandY1 to bandY1 + ahead / 2)
        val width = frame.width()
        builder.execute {
            val todo = ranges.filter { it.second > 0 }.flatMap { (a, b) -> runCatching { frame.rowsIn(a, b) }.getOrDefault(emptyList()) }
            val built = todo.mapNotNull { p ->
                val k = ModelKey(p.key.toLong(), p.version.toLong(), width)
                runCatching { frame.display(p.index) }.getOrNull()?.let { k to RowPaint(it, paints) }
            }
            main.post {
                if (!closed) for ((k, m) in built) if (!cache.containsKey(k)) cache[k] = m
            }
        }
    }

    // ── horizontal scrollers ─────────────────────────────────────────────

    private val scrollerX = HashMap<Long, FloatArray>()

    private fun scrollerOffsets(key: Long, model: RowPaint): FloatArray {
        val n = model.display.scrollers.size
        val arr = scrollerX[key]?.takeIf { it.size == n } ?: FloatArray(n).also { scrollerX[key] = it }
        model.display.scrollers.forEachIndexed { i, s -> arr[i] = arr[i].coerceIn(0f, max(0f, s.contentWidth - s.w)) }
        return arr
    }

    // ── touch ────────────────────────────────────────────────────────────

    private val fling = OverScroller(context)
    private val hFling = OverScroller(context)
    private var velocity: VelocityTracker? = null
    private val slop = ViewConfiguration.get(context).scaledTouchSlop
    private val minFling = ViewConfiguration.get(context).scaledMinimumFlingVelocity
    private val maxFling = ViewConfiguration.get(context).scaledMaximumFlingVelocity
    private var downX = 0f
    private var downY = 0f
    private var lastY = 0f
    private var lastX = 0f
    private var dragging = false
    private var hDrag: Pair<Long, Int>? = null
    private var hTarget: Pair<Long, Int>? = null
    private var lastDirection = 1
    private var longPressed = false
    private val topEdge = EdgeEffect(context)
    private val bottomEdge = EdgeEffect(context)

    private val longPress = Runnable {
        if (dragging) return@Runnable
        longPressed = true
        val hit = rowAt(downY) ?: return@Runnable
        val frame = current ?: return@Runnable
        performHapticFeedback(android.view.HapticFeedbackConstants.LONG_PRESS)
        host?.longPress(hit.first.index, hit.second.display.copyText, frame.messageText(hit.first.index))
    }

    /** Row under a view-space y (px). */
    private fun rowAt(yPx: Float): Pair<RowPlacement, RowPaint>? {
        val y = yPx / density + scroll
        for (p in band) {
            if (y >= p.y && y < p.y + p.height) {
                val m = shown[p.key.toLong()] ?: return null
                return p to m
            }
        }
        return null
    }

    /** The horizontal scroller under a point, if its content overflows. */
    private fun scrollerAt(xPx: Float, yPx: Float): Pair<Long, Int>? {
        val (p, m) = rowAt(yPx) ?: return null
        val x = xPx / density
        val y = yPx / density + scroll - p.y
        m.display.scrollers.forEachIndexed { i, s ->
            if (x >= s.x && x < s.x + s.w && y >= s.y && y < s.y + s.h && s.contentWidth > s.w + 0.5f) return p.key.toLong() to i
        }
        return null
    }

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(e: MotionEvent): Boolean {
        if (velocity == null) velocity = VelocityTracker.obtain()
        velocity!!.addMovement(e)
        when (e.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                val wasFlinging = !fling.isFinished || springing
                fling.forceFinished(true)
                hFling.forceFinished(true)
                downX = e.x; downY = e.y; lastX = e.x; lastY = e.y
                dragging = false
                longPressed = false
                hDrag = null
                hTarget = scrollerAt(e.x, e.y)
                if (wasFlinging) {
                    // A touch that stops a fling isn't a tap.
                    dragging = true
                    setFollowing(false)
                } else {
                    postDelayed(longPress, ViewConfiguration.getLongPressTimeout().toLong())
                }
            }
            MotionEvent.ACTION_MOVE -> {
                val dx = e.x - downX
                val dy = e.y - downY
                if (!dragging && hDrag == null) {
                    if (hTarget != null && abs(dx) > slop && abs(dx) > abs(dy)) {
                        hDrag = hTarget
                        removeCallbacks(longPress)
                        parent?.requestDisallowInterceptTouchEvent(true)
                    } else if (abs(dy) > slop) {
                        dragging = true
                        removeCallbacks(longPress)
                        setFollowing(false)
                        parent?.requestDisallowInterceptTouchEvent(true)
                        lastY = e.y
                    }
                }
                hDrag?.let { (key, i) ->
                    scrollerX[key]?.let { arr -> arr[i] -= (e.x - lastX) / density }
                    lastX = e.x
                    invalidate()
                }
                if (dragging) {
                    val delta = (lastY - e.y) / density
                    lastY = e.y
                    if (delta != 0f) lastDirection = if (delta > 0) 1 else -1
                    val next = scroll + delta
                    when {
                        next < minScroll -> {
                            topEdge.onPullDistance(-delta * density / height, e.x / width)
                            scroll = minScroll
                        }
                        next > maxScroll -> {
                            bottomEdge.onPullDistance(delta * density / height, 1f - e.x / width)
                            scroll = maxScroll
                        }
                        else -> scroll = next
                    }
                    lastScrollMs = SystemClock.uptimeMillis()
                    invalidate()
                    report()
                }
            }
            MotionEvent.ACTION_UP -> {
                removeCallbacks(longPress)
                velocity!!.computeCurrentVelocity(1000, maxFling.toFloat())
                val vy = velocity!!.yVelocity
                val vx = velocity!!.xVelocity
                when {
                    hDrag != null -> {
                        val (key, i) = hDrag!!
                        flingScroller(key, i, vx)
                    }
                    dragging -> {
                        releaseEdges()
                        if (abs(vy) > minFling) {
                            fling.fling(0, (scroll * density).toInt(), 0, -vy.toInt(), 0, 0, (minScroll * density).toInt(), (maxScroll * density).toInt())
                            lastDirection = if (vy < 0) 1 else -1
                            postInvalidateOnAnimation()
                        }
                        // Releasing near the tail while not flinging away hands control back.
                        if (distanceFromBottom < 70 && vy <= 50 * density) setFollowing(true)
                        if (following) startSpring()
                    }
                    !longPressed -> handleTap(e.x, e.y)
                }
                dragging = false
                hDrag = null
                velocity?.recycle()
                velocity = null
            }
            MotionEvent.ACTION_CANCEL -> {
                removeCallbacks(longPress)
                releaseEdges()
                dragging = false
                hDrag = null
                velocity?.recycle()
                velocity = null
            }
        }
        return true
    }

    private fun releaseEdges() {
        topEdge.onRelease()
        bottomEdge.onRelease()
        if (!topEdge.isFinished || !bottomEdge.isFinished) postInvalidateOnAnimation()
    }

    private fun drawEdges(canvas: Canvas) {
        if (!topEdge.isFinished) {
            canvas.save()
            canvas.translate(0f, topInset * density)
            topEdge.setSize(width, height)
            if (topEdge.draw(canvas)) postInvalidateOnAnimation()
            canvas.restore()
        }
        if (!bottomEdge.isFinished) {
            canvas.save()
            canvas.translate(width.toFloat(), height - bottomInset * density)
            canvas.rotate(180f)
            bottomEdge.setSize(width, height)
            if (bottomEdge.draw(canvas)) postInvalidateOnAnimation()
            canvas.restore()
        }
    }

    /** Momentum carrying the list back into the tail re-engages following. */
    private fun onFlingStep() {
        lastScrollMs = SystemClock.uptimeMillis()
        if (!following && lastDirection > 0 && distanceFromBottom < 70) setFollowing(true)
        report()
        if (fling.isFinished && following) startSpring()
    }

    private fun flingScroller(key: Long, i: Int, vx: Float) {
        val m = shown[key] ?: return
        val s = m.display.scrollers.getOrNull(i) ?: return
        val arr = scrollerX[key] ?: return
        if (abs(vx) < minFling) return
        hFling.fling((arr[i] * density).toInt(), 0, -vx.toInt(), 0, 0, ((s.contentWidth - s.w) * density).toInt(), 0, 0)
        val tick = object : Runnable {
            override fun run() {
                if (!hFling.computeScrollOffset()) return
                scrollerX[key]?.let { it[i] = hFling.currX / density }
                invalidate()
                postOnAnimation(this)
            }
        }
        postOnAnimation(tick)
    }

    private fun handleTap(xPx: Float, yPx: Float) {
        val hit = rowAt(yPx)
        if (hit == null) {
            host?.tapEmpty()
            return
        }
        val (p, m) = hit
        val x = xPx / density
        val y = yPx / density + scroll - p.y
        val offsets = scrollerOffsets(p.key.toLong(), m)
        // Controls first (disclosures, tool rows, copy, images).
        val w = widgets.hit(m, x, y, offsets)
        if (w != null && handleWidget(p, m, w)) return
        for (l in m.display.links) {
            var lx = x
            var ly = y
            l.scroller?.let { si ->
                val s = m.display.scrollers[si.toInt()]
                lx = x - s.x + offsets[si.toInt()]
                ly = y - s.y
            }
            if (lx >= l.x - 4 && lx <= l.x + l.w + 4 && ly >= l.y - 2 && ly <= l.y + l.h + 2) {
                host?.openLink(l.url)
                return
            }
        }
        host?.tapEmpty()
    }

    private fun handleWidget(p: RowPlacement, m: RowPaint, w: Widget): Boolean {
        val h = host ?: return false
        when (val k = w.kind) {
            is uniffi.zeron_core.WidgetKind.Disclosure -> {
                performHapticFeedback(android.view.HapticFeedbackConstants.CLOCK_TICK)
                h.toggle(m.key)
            }
            is uniffi.zeron_core.WidgetKind.Chevron -> h.toggle(m.key)
            is uniffi.zeron_core.WidgetKind.ToolToggle -> {
                performHapticFeedback(android.view.HapticFeedbackConstants.CLOCK_TICK)
                h.toggleDetail(m.key, k.detail, k.open)
            }
            uniffi.zeron_core.WidgetKind.CopyCode -> {
                widgets.markCopied(m.key, w.id)
                h.copied(w.payload ?: "")
                invalidate()
            }
            is uniffi.zeron_core.WidgetKind.Detail -> h.showDetail(k.title, w.payload ?: "")
            is uniffi.zeron_core.WidgetKind.Image -> h.openImage(k.reference, widgets.images[k.reference])
            else -> return false
        }
        return true
    }

    /** A disclosure the host toggled programmatically / images that loaded. */
    fun refresh() = invalidate()

    fun close() {
        closed = true
        widgets.close()
        builder.shutdownNow()
        stopSpring()
        shown.values.forEach { it.release() }
        cache.values.forEach { it.release() }
        shown.clear()
        cache.clear()
    }

    /** Theme switch: recorded nodes carry colors — re-record lazily. */
    fun themeChanged() {
        invalidate()
    }
}
