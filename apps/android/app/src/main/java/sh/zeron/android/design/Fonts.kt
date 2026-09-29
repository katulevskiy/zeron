package sh.zeron.android.design

import android.content.Context
import android.graphics.Paint
import android.graphics.Typeface
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import uniffi.zeron_core.FaceData
import uniffi.zeron_core.FaceRole
import uniffi.zeron_core.PlatformMeasurer
import uniffi.zeron_core.TextSystem
import java.util.concurrent.ConcurrentHashMap

/**
 * The bundled Geist faces. Rust measures the exact bytes Skia draws with
 * (`assets/fonts`, the same files the iOS app ships), so measurement and
 * rendering share one source of truth.
 */
object Fonts {
    private val files = linkedMapOf(
        FaceRole.SANS to "Geist",
        FaceRole.SANS_MEDIUM to "Geist-Medium",
        FaceRole.SANS_SEMIBOLD to "Geist-SemiBold",
        FaceRole.SANS_BOLD to "Geist-Bold",
        FaceRole.SANS_ITALIC to "Geist-Italic",
        FaceRole.SANS_MEDIUM_ITALIC to "Geist-MediumItalic",
        FaceRole.SANS_SEMIBOLD_ITALIC to "Geist-SemiBoldItalic",
        FaceRole.SANS_BOLD_ITALIC to "Geist-BoldItalic",
        FaceRole.MONO to "GeistMono",
        FaceRole.MONO_MEDIUM to "GeistMono-Medium",
        FaceRole.MONO_SEMIBOLD to "GeistMono-SemiBold",
        FaceRole.MONO_ITALIC to "GeistMono-Italic",
    )

    private fun path(role: FaceRole) = "fonts/${files.getValue(role)}.ttf"

    private lateinit var app: Context
    private val typefaces = ConcurrentHashMap<FaceRole, Typeface>()

    fun init(context: Context) {
        app = context.applicationContext
    }

    fun typeface(role: FaceRole): Typeface =
        typefaces.getOrPut(role) { Typeface.createFromAsset(app.assets, path(role)) }

    /** Face bytes for Rust (read once: the text system is process-wide). */
    fun faceData(): List<FaceData> = files.keys.map { role ->
        FaceData(role, app.assets.open(path(role)).use { it.readBytes() })
    }

    /** One process-wide text system shared by every transcript. */
    val textSystem: TextSystem by lazy { TextSystem(faceData(), SkiaMeasurer) }

    val sans: FontFamily by lazy {
        val a = app.assets
        FontFamily(
            Font("fonts/Geist.ttf", a, FontWeight.Normal),
            Font("fonts/Geist-Medium.ttf", a, FontWeight.Medium),
            Font("fonts/Geist-SemiBold.ttf", a, FontWeight.SemiBold),
            Font("fonts/Geist-Bold.ttf", a, FontWeight.Bold),
            Font("fonts/Geist-Italic.ttf", a, FontWeight.Normal, FontStyle.Italic),
            Font("fonts/Geist-MediumItalic.ttf", a, FontWeight.Medium, FontStyle.Italic),
            Font("fonts/Geist-SemiBoldItalic.ttf", a, FontWeight.SemiBold, FontStyle.Italic),
            Font("fonts/Geist-BoldItalic.ttf", a, FontWeight.Bold, FontStyle.Italic),
        )
    }

    val mono: FontFamily by lazy {
        val a = app.assets
        FontFamily(
            Font("fonts/GeistMono.ttf", a, FontWeight.Normal),
            Font("fonts/GeistMono-Medium.ttf", a, FontWeight.Medium),
            Font("fonts/GeistMono-SemiBold.ttf", a, FontWeight.SemiBold),
            Font("fonts/GeistMono-Italic.ttf", a, FontWeight.Normal, FontStyle.Italic),
        )
    }

    /**
     * A paint configured exactly as layout measured: unhinted linear
     * advances (rustybuzz knows nothing of hinting), ligatures only where
     * the style asked for them — code keeps `->`/`!=` as typed.
     */
    fun paint(role: FaceRole, size: Float, ligatures: Boolean): Paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        typeface = typeface(role)
        textSize = size
        isSubpixelText = true
        isLinearText = true
        hinting = Paint.HINTING_OFF
        if (!ligatures) fontFeatureSettings = "'liga' 0, 'clig' 0, 'calt' 0"
    }
}

/**
 * Skia (via android.graphics) as ground truth for glyphs the bundled faces
 * can't render — emoji, CJK. Android falls back through the system fonts
 * exactly as it will when painting. Called on the Rust layout thread.
 */
object SkiaMeasurer : PlatformMeasurer {
    private val paints = ThreadLocal.withInitial { HashMap<Long, Paint>() }

    private fun paint(face: FaceRole, size: Float, ligatures: Boolean): Paint {
        val key = (face.ordinal.toLong() shl 40) or ((size * 100).toLong() shl 1) or (if (ligatures) 1L else 0L)
        return paints.get()!!.getOrPut(key) { Fonts.paint(face, size, ligatures) }
    }

    override fun measure(face: FaceRole, size: Float, ligatures: Boolean, text: String): Float =
        paint(face, size, ligatures).measureText(text)

    /**
     * Per-scalar advances of `text` shaped as one run (fallback font and
     * kerning chosen in context): a cluster's width lands on its first
     * UTF-16 unit, which folds onto its scalar.
     */
    override fun measureRun(face: FaceRole, size: Float, ligatures: Boolean, text: String): List<Float> {
        val n = text.length
        if (n == 0) return emptyList()
        val perUnit = FloatArray(n)
        paint(face, size, ligatures).getTextRunAdvances(text.toCharArray(), 0, n, 0, n, false, perUnit, 0)
        val out = ArrayList<Float>(text.codePointCount(0, n))
        var i = 0
        while (i < n) {
            val cp = text.codePointAt(i)
            val units = Character.charCount(cp)
            var w = 0f
            for (k in i until minOf(n, i + units)) w += perUnit[k]
            out.add(w)
            i += units
        }
        return out
    }
}
