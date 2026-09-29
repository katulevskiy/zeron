package sh.zeron.android.ui.onboarding

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppMode
import sh.zeron.android.core.AuthStage
import sh.zeron.android.design.ButtonStyle
import sh.zeron.android.design.CapsuleButton
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.GlyphIcon
import sh.zeron.android.design.GlyphKind
import sh.zeron.android.design.LocalColors
import sh.zeron.android.design.LocalOverlays
import sh.zeron.android.design.StatusGlyph
import sh.zeron.android.design.ZText
import sh.zeron.android.design.ZType
import sh.zeron.android.design.glass
import sh.zeron.android.design.pressable
import sh.zeron.android.ui.LocalActivity
import sh.zeron.android.ui.LocalModel
import sh.zeron.android.ui.openUrl
import sh.zeron.android.ui.settings.RuntimeCard
import sh.zeron.android.ui.settings.detail
import sh.zeron.android.ui.settings.glyph
import sh.zeron.android.ui.settings.startEngine
import sh.zeron.android.ui.settings.title
import sh.zeron.runtime.RuntimeState

/** Centered column for the gate screens (onboarding, sign-in, phone setup). */
@Composable
private fun GateColumn(content: @Composable () -> Unit) {
    Box(
        Modifier
            .fillMaxSize()
            .windowInsetsPadding(WindowInsets.systemBars),
        contentAlignment = Alignment.TopCenter,
    ) {
        Column(
            Modifier
                .widthIn(max = 520.dp)
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 24.dp, vertical = 16.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) { content() }
    }
}

@Composable
private fun Hero(glyph: Glyph, title: String, tagline: String) {
    val c = LocalColors.current
    Box(Modifier.size(72.dp).background(c.accentSoft, RoundedCornerShape(22.dp)), contentAlignment = Alignment.Center) {
        GlyphIcon(glyph, c.accent, size = 38.dp, weight = 1.4f)
    }
    Spacer(Modifier.height(18.dp))
    ZText(title, ZType.sans(32f, FontWeight.SemiBold), c.text, align = TextAlign.Center)
    Spacer(Modifier.height(8.dp))
    ZText(tagline, ZType.sans(16.5f, lineHeight = 23f), c.secondary, align = TextAlign.Center)
}

/** First run: where do agents run? (Switchable later in Settings.) */
@Composable
fun OnboardingScreen() {
    val model = LocalModel.current
    val c = LocalColors.current
    GateColumn {
        Spacer(Modifier.height(56.dp))
        Hero(Glyph.Sparkle, "Zeron", "Your coding agents, from anywhere.")
        Spacer(Modifier.height(40.dp))
        for (m in listOf(AppMode.Phone, AppMode.Account, AppMode.Demo)) {
            val enabled = m != AppMode.Phone || model.phone.isSupportedAbi
            ModeCard(m, enabled) { model.chooseMode(m) }
            Spacer(Modifier.height(10.dp))
        }
        Spacer(Modifier.height(10.dp))
        ZText("You can switch any time in Settings.", ZType.sans(13.5f), c.tertiary, align = TextAlign.Center)
    }
}

@Composable
private fun ModeCard(mode: AppMode, enabled: Boolean, onClick: () -> Unit) {
    val c = LocalColors.current
    Row(
        Modifier
            .fillMaxWidth()
            .graphicsLayer { alpha = if (enabled) 1f else 0.45f }
            .glass(c, RoundedCornerShape(22.dp), 4.dp)
            .pressable(enabled = enabled, highlight = c.controlFill, dim = 1f, shape = RoundedCornerShape(22.dp), label = mode.title(), onClick = onClick)
            .padding(16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(44.dp).background(c.accentSoft, RoundedCornerShape(14.dp)), contentAlignment = Alignment.Center) {
            GlyphIcon(mode.glyph(), c.accent, size = 22.dp)
        }
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            ZText(mode.title(), ZType.sans(17f, FontWeight.SemiBold), c.text)
            ZText(if (enabled) mode.detail() else "Not available on this device's CPU", ZType.sans(14f), c.secondary, maxLines = 2)
        }
        GlyphIcon(Glyph.ChevronRight, c.tertiary, size = 15.dp)
    }
}

/** Account mode without a session: WorkOS in a Custom Tab, then the org picker. */
@Composable
fun SignInScreen() {
    val model = LocalModel.current
    val activity = LocalActivity.current
    val c = LocalColors.current
    val auth by model.auth.collectAsState()
    fun start() = openUrl(activity, model.beginSignIn(), c.backgroundArgb)
    GateColumn {
        Spacer(Modifier.height(56.dp))
        Hero(Glyph.Person, "Sign in", "Drive the agents on your computers and servers from this phone.")
        Spacer(Modifier.height(40.dp))
        when (val a = auth) {
            AuthStage.Exchanging -> Row(verticalAlignment = Alignment.CenterVertically) {
                StatusGlyph(GlyphKind.Spinner, size = 16.dp)
                Spacer(Modifier.width(10.dp))
                ZText("Signing in…", ZType.sans(16f), c.secondary)
            }
            is AuthStage.PickOrg -> {
                ZText("Choose an organization", ZType.sans(15f, FontWeight.Medium), c.secondary)
                Spacer(Modifier.height(12.dp))
                for (org in a.orgs) {
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .padding(vertical = 4.dp)
                            .glass(c, RoundedCornerShape(18.dp), 3.dp)
                            .pressable(highlight = c.controlFill, dim = 1f, shape = RoundedCornerShape(18.dp)) { model.pickOrg(org) }
                            .padding(16.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        ZText(org.name, ZType.sans(16.5f, FontWeight.Medium), c.text, Modifier.weight(1f))
                        GlyphIcon(Glyph.ChevronRight, c.tertiary, size = 14.dp)
                    }
                }
                Spacer(Modifier.height(10.dp))
                CapsuleButton("Cancel", style = ButtonStyle.Ghost, compact = true) { model.cancelSignIn() }
            }
            AuthStage.Browser -> {
                ZText("Finish signing in in the browser.", ZType.sans(15.5f), c.secondary, align = TextAlign.Center)
                Spacer(Modifier.height(16.dp))
                CapsuleButton("Open Sign-In Again", Modifier.fillMaxWidth(), style = ButtonStyle.Secondary) { start() }
                Spacer(Modifier.height(8.dp))
                CapsuleButton("Cancel", style = ButtonStyle.Ghost, compact = true) { model.cancelSignIn() }
            }
            else -> {
                if (a is AuthStage.Failed) {
                    ZText(a.message, ZType.sans(15f), c.danger, align = TextAlign.Center)
                    Spacer(Modifier.height(14.dp))
                }
                CapsuleButton("Sign In with Zeron", Modifier.fillMaxWidth()) { start() }
            }
        }
        Spacer(Modifier.height(24.dp))
        CapsuleButton("Use Zeron another way", style = ButtonStyle.Ghost, compact = true) { model.resetMode() }
    }
}

/** This-phone mode before the engine answers: set up / start / progress / failure. */
@Composable
fun PhoneSetupScreen() {
    val model = LocalModel.current
    val activity = LocalActivity.current
    val overlays = LocalOverlays.current
    val c = LocalColors.current
    val scope = rememberCoroutineScope()
    val phone = model.phone
    val state by phone.state.collectAsState()
    // Relaunch: the user ran the engine before — bring it back without a tap.
    LaunchedEffect(Unit) {
        if (model.prefs.phoneAutostart && phone.state.value == RuntimeState.Stopped) phone.start()
    }
    GateColumn {
        Spacer(Modifier.height(40.dp))
        Hero(Glyph.Phone, "Agents on this phone", "Zeron runs its engine and real coding agents in a small Linux system on this device.")
        Spacer(Modifier.height(28.dp))
        Box(Modifier.fillMaxWidth().padding(horizontal = 0.dp)) { RuntimeCard(phone, state) }
        Spacer(Modifier.height(16.dp))
        when (state) {
            RuntimeState.NotInstalled -> CapsuleButton("Set Up", Modifier.fillMaxWidth(), enabled = phone.isSupportedAbi) { startEngine(model, activity) }
            RuntimeState.Stopped -> CapsuleButton("Start Engine", Modifier.fillMaxWidth(), glyph = Glyph.Power) { startEngine(model, activity) }
            is RuntimeState.Failed -> {
                CapsuleButton("Try Again", Modifier.fillMaxWidth(), glyph = Glyph.Refresh) { startEngine(model, activity) }
                Spacer(Modifier.height(8.dp))
                CapsuleButton("Reset Engine…", Modifier.fillMaxWidth(), style = ButtonStyle.Destructive) {
                    overlays.confirm("Reset the on-device engine?", "Deletes the Linux guest and everything in it, then sets it up fresh.", "Reset", destructive = true) {
                        scope.launch { phone.reset() }
                    }
                }
                Spacer(Modifier.height(14.dp))
                Box(
                    Modifier
                        .fillMaxWidth()
                        .heightIn(max = 220.dp)
                        .background(c.controlFill, RoundedCornerShape(12.dp))
                        .verticalScroll(rememberScrollState(Int.MAX_VALUE))
                        .horizontalScroll(rememberScrollState())
                        .padding(10.dp),
                ) {
                    ZText((state as RuntimeState.Failed).logTail.ifBlank { phone.logTail(60) }, ZType.mono(11f), c.secondary, overflow = TextOverflow.Clip)
                }
            }
            is RuntimeState.Running -> Row(verticalAlignment = Alignment.CenterVertically) {
                StatusGlyph(GlyphKind.Spinner)
                Spacer(Modifier.width(10.dp))
                ZText("Connecting to the engine…", ZType.sans(15f), c.secondary)
            }
            else -> CapsuleButton("Stop", Modifier.fillMaxWidth(), style = ButtonStyle.Secondary) { phone.stop() }
        }
        Spacer(Modifier.height(24.dp))
        Row(horizontalArrangement = Arrangement.Center, modifier = Modifier.fillMaxWidth()) {
            CapsuleButton("Use Zeron another way", style = ButtonStyle.Ghost, compact = true) { model.resetMode() }
        }
    }
}
