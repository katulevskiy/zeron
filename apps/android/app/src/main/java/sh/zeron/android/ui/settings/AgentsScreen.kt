package sh.zeron.android.ui.settings

import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject
import sh.zeron.android.core.Agents
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.userMessage
import sh.zeron.android.design.BrandMark
import sh.zeron.android.design.ButtonStyle
import sh.zeron.android.design.CapsuleButton
import sh.zeron.android.design.Chip
import sh.zeron.android.design.EmptyState
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.GlyphIcon
import sh.zeron.android.design.GlyphKind
import sh.zeron.android.design.LocalColors
import sh.zeron.android.design.LocalOverlays
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.Overlays
import sh.zeron.android.design.ProgressTrack
import sh.zeron.android.design.StatusGlyph
import sh.zeron.android.design.TopBar
import sh.zeron.android.design.ZText
import sh.zeron.android.design.ZTextField
import sh.zeron.android.design.ZType
import sh.zeron.android.design.anchor
import sh.zeron.android.design.glass
import sh.zeron.android.design.rememberAnchor
import sh.zeron.android.ui.LocalModel
import sh.zeron.android.ui.LocalNavigator
import sh.zeron.android.ui.openUrl
import sh.zeron.android.ui.session.copy
import uniffi.zeron_core.DeviceView

/** Install progress for one harness (installs run minutes; the relay call may time out first). */
private data class Install(val job: Job, val startedAt: Long)

/**
 * Settings → Agents: the harnesses on an engine device with their install
 * state (Install / progress / Cancel) and agent accounts (browser or
 * paste-code sign-in, sign out). Everything goes over `host_call` to that
 * device's engine; Demo answers from a simulated engine.
 */
@Composable
fun AgentsScreen() {
    val model = LocalModel.current
    val nav = LocalNavigator.current
    val overlays = LocalOverlays.current
    val c = LocalColors.current
    val scope = rememberCoroutineScope()
    val client = model.client.value
    val devices = remember(client) { model.executionDevices() }
    var device by remember { mutableStateOf(devices.firstOrNull { it.online } ?: devices.firstOrNull()) }
    var harnesses by remember { mutableStateOf<List<Agents.Harness>?>(null) }
    var accounts by remember { mutableStateOf(Agents.Accounts(emptyList(), emptyMap())) }
    var versions by remember { mutableStateOf<Map<String, Agents.Version>>(emptyMap()) }
    var error by remember { mutableStateOf<String?>(null) }
    val installs = remember { mutableStateMapOf<String, Install>() }
    var reloads by remember { mutableIntStateOf(0) }
    val deviceAnchor = rememberAnchor()

    LaunchedEffect(device, reloads, client) {
        val d = device ?: return@LaunchedEffect
        error = null
        val list = async { runCatching { Agents.harnesses(model.hostCall(d.id, Agents.LIST_HARNESSES)) } }
        val accts = async { runCatching { Agents.accounts(model.hostCall(d.id, Agents.LIST_ACCOUNTS)) } }
        val vers = async { runCatching { Agents.versions(model.hostCall(d.id, Agents.CHECK_UPDATES)) } }
        list.await().onSuccess { harnesses = it }.onFailure {
            harnesses = harnesses ?: emptyList()
            error = it.userMessage()
        }
        accts.await().onSuccess { accounts = it }
        vers.await().onSuccess { versions = it }
    }

    fun install(h: Agents.Harness) {
        val d = device ?: return
        val job = scope.launch {
            try {
                val reply = model.hostCall(d.id, Agents.INSTALL_HARNESS, JSONObject().put("harness", h.id))
                harnesses = Agents.harnesses(reply)
                overlays.toast("${h.name} installed")
            } catch (e: Exception) {
                if (e is kotlinx.coroutines.CancellationException) throw e
                val msg = e.userMessage()
                if (e is uniffi.zeron_core.CoreException.HostUnavailable && Agents.isTimeout(e.detail)) {
                    // Still installing on the device: watch the catalog instead.
                    if (awaitInstalled(model, d.id, h.id)) {
                        overlays.toast("${h.name} installed")
                    } else {
                        overlays.alert("Couldn't install ${h.name}", "The install didn't finish in time. Check the engine log.")
                    }
                } else if (!msg.contains("cancelled", ignoreCase = true)) {
                    overlays.alert("Couldn't install ${h.name}", msg)
                }
            } finally {
                installs.remove(h.id)
                reloads++
            }
        }
        installs[h.id] = Install(job, System.currentTimeMillis())
    }

    fun cancelInstall(h: Agents.Harness) {
        val d = device ?: return
        scope.launch { runCatching { model.hostCall(d.id, Agents.CANCEL_INSTALL, JSONObject().put("harness", h.id)) } }
    }

    Column(Modifier.fillMaxSize()) {
        TopBar("Coding agents", subtitle = device?.name, onBack = { nav.pop() })
        if (client == null || device == null) {
            EmptyState(Glyph.Terminal, "No engine to manage", "Agents install on a device running Zeron — this phone's engine or one of your computers.", Modifier.fillMaxWidth().padding(top = 60.dp))
            return@Column
        }
        if (devices.size > 1) {
            Row(Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) {
                Chip(device!!.name, Modifier.anchor(deviceAnchor), leading = { GlyphIcon(Glyph.Desktop, c.secondary, size = 15.dp) }) {
                    overlays.menu(deviceAnchor, "Device", devices.map { d: DeviceView ->
                        MenuEntry.Item(d.name, subtitle = if (d.online) "Online" else "Offline", checked = d.id == device?.id) {
                            device = d
                            harnesses = null
                        }
                    })
                }
            }
        }
        val list = harnesses
        when {
            list == null -> Box(Modifier.fillMaxWidth().padding(top = 80.dp), contentAlignment = Alignment.Center) { StatusGlyph(GlyphKind.Spinner, size = 18.dp) }
            list.isEmpty() -> EmptyState(Glyph.Warning, "Couldn't load the agents", error ?: "The device didn't answer.", Modifier.fillMaxWidth().padding(top = 60.dp)) {
                CapsuleButton("Try Again", compact = true, style = ButtonStyle.Secondary) { reloads++ }
            }
            else -> LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 8.dp, bottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() + 24.dp),
            ) {
                items(list.sortedByDescending { it.installed }, key = { it.id }) { h ->
                    HarnessCard(
                        h,
                        accounts.forHarness(h.id),
                        accounts.warnings[h.id],
                        versions[h.id],
                        installs[h.id],
                        onInstall = { install(h) },
                        onCancel = { cancelInstall(h) },
                        onSignIn = { signIn(model, overlays, device!!.id, h) { reloads++ } },
                        onSignOut = { a ->
                            overlays.confirm("Sign out of ${a.title}?", "${h.name} on ${device!!.name} forgets this account.", "Sign Out", destructive = true) {
                                scope.launch {
                                    runCatching {
                                        accounts = Agents.accounts(model.hostCall(device!!.id, Agents.FORGET_ACCOUNT, JSONObject().put("harness", h.id).put("accountId", a.id)))
                                    }.onFailure { overlays.alert("Couldn't sign out", it.userMessage()) }
                                }
                            }
                        },
                    )
                    Spacer(Modifier.height(10.dp))
                }
            }
        }
    }
}

/** Poll the catalog until `harness` reports installed (≤ 15 min, the engine's own limit). */
private suspend fun awaitInstalled(model: AppModel, device: String, harness: String): Boolean {
    val deadline = System.currentTimeMillis() + 15 * 60_000
    while (System.currentTimeMillis() < deadline) {
        delay(4000)
        val list = runCatching { Agents.harnesses(model.hostCall(device, Agents.LIST_HARNESSES)) }.getOrNull() ?: continue
        if (list.firstOrNull { it.id == harness }?.installed == true) return true
    }
    return false
}

@Composable
private fun HarnessCard(
    h: Agents.Harness,
    accounts: List<Agents.Account>,
    warning: String?,
    version: Agents.Version?,
    install: Install?,
    onInstall: () -> Unit,
    onCancel: () -> Unit,
    onSignIn: () -> Unit,
    onSignOut: (Agents.Account) -> Unit,
) {
    val c = LocalColors.current
    Column(
        Modifier
            .fillMaxWidth()
            .background(c.elevated, RoundedCornerShape(20.dp))
            .animateContentSize()
            .padding(16.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(40.dp).background(c.controlFill, RoundedCornerShape(12.dp)), contentAlignment = Alignment.Center) {
                BrandMark(h.id, 22.dp)
            }
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                ZText(h.name, ZType.sans(16.5f, FontWeight.SemiBold), c.text, maxLines = 1)
                val status = when {
                    install != null -> "Installing…"
                    h.installed -> listOfNotNull("Installed", version?.installed?.let { "v$it" }).joinToString(" · ")
                    h.canInstall -> "Not installed"
                    else -> "Not installed · install it on the device"
                }
                ZText(status, ZType.sans(13.5f), if (h.installed) c.secondary else c.tertiary, maxLines = 1)
            }
            when {
                install != null -> CapsuleButton("Cancel", style = ButtonStyle.Secondary, compact = true, onClick = onCancel)
                !h.installed && h.canInstall -> CapsuleButton("Install", style = ButtonStyle.Accent, glyph = Glyph.Download, compact = true, onClick = onInstall)
            }
        }
        if (install != null) {
            Spacer(Modifier.height(12.dp))
            ProgressTrack(null)
        }
        if (h.installed) {
            if (accounts.isNotEmpty()) Spacer(Modifier.height(12.dp))
            for (a in accounts) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .padding(vertical = 4.dp)
                        .background(c.controlFill, RoundedCornerShape(14.dp))
                        .padding(start = 12.dp, end = 6.dp, top = 6.dp, bottom = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    GlyphIcon(Glyph.Person, c.secondary, size = 17.dp)
                    Spacer(Modifier.width(10.dp))
                    Column(Modifier.weight(1f)) {
                        ZText(a.title, ZType.sans(14.5f, FontWeight.Medium), c.text, maxLines = 1)
                        ZText(listOfNotNull(a.plan, if (a.active) "Active" else null).joinToString(" · ").ifEmpty { "Signed in" }, ZType.sans(12.5f), c.secondary, maxLines = 1)
                    }
                    CapsuleButton("Sign Out", style = ButtonStyle.Ghost, compact = true) { onSignOut(a) }
                }
            }
            if (warning != null) {
                Spacer(Modifier.height(8.dp))
                ZText(warning, ZType.sans(12.5f), c.warning)
            }
            Spacer(Modifier.height(10.dp))
            CapsuleButton(if (accounts.isEmpty()) "Sign In" else "Add Account", Modifier.fillMaxWidth(), style = ButtonStyle.Secondary, glyph = Glyph.Key, compact = true, onClick = onSignIn)
        }
    }
}

/**
 * An agent sign-in: open the page the engine hands back; browser logins
 * finish on their own (poll), paste-code logins take the code here.
 */
private fun signIn(model: AppModel, overlays: Overlays, device: String, h: Agents.Harness, done: () -> Unit) {
    overlays.sheet { dismiss ->
        val c = LocalColors.current
        val context = LocalContext.current
        val scope = rememberCoroutineScope()
        var start by remember { mutableStateOf<Agents.LoginStart?>(null) }
        var url by remember { mutableStateOf("") }
        var failure by remember { mutableStateOf<String?>(null) }
        var code by remember { mutableStateOf("") }
        var busy by remember { mutableStateOf(false) }
        var finished by remember { mutableStateOf(false) }

        fun complete() {
            finished = true
            done()
            dismiss()
            overlays.toast("Signed in to ${h.name}")
        }

        LaunchedEffect(Unit) {
            val s = runCatching { Agents.loginStart(model.hostCall(device, Agents.START_LOGIN, JSONObject().put("harness", h.id))) }
                .onFailure { failure = it.userMessage() }.getOrNull() ?: return@LaunchedEffect run { if (failure == null) failure = "The device didn't start a sign-in." }
            start = s
            url = s.url
            // Demo sign-in pages are placeholders: nothing to open.
            if (url.isNotEmpty() && !model.isDemo) openUrl(context, url)
            if (s.mode == Agents.LoginMode.Browser) {
                while (!finished) {
                    delay(2000)
                    when (val p = runCatching { Agents.loginPoll(model.hostCall(device, Agents.POLL_LOGIN, JSONObject().put("loginId", s.loginId))) }.getOrElse { Agents.LoginPoll.Pending(null) }) {
                        Agents.LoginPoll.Done -> return@LaunchedEffect complete()
                        is Agents.LoginPoll.Failed -> {
                            failure = p.message
                            return@LaunchedEffect
                        }
                        is Agents.LoginPoll.Pending -> if (p.url != null && url.isEmpty()) {
                            url = p.url
                            if (!model.isDemo) openUrl(context, p.url)
                        }
                    }
                }
            }
        }
        androidx.compose.runtime.DisposableEffect(Unit) {
            onDispose {
                val s = start
                if (!finished && s != null) model.scope.launch { runCatching { model.hostCall(device, Agents.CANCEL_LOGIN, JSONObject().put("loginId", s.loginId)) } }
            }
        }

        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 18.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                BrandMark(h.id, 24.dp)
                Spacer(Modifier.width(10.dp))
                ZText("Sign in to ${h.name}", ZType.sans(19f, FontWeight.SemiBold), c.text)
            }
            Spacer(Modifier.height(14.dp))
            val s = start
            when {
                failure != null -> ZText(failure!!, ZType.sans(15f), c.danger)
                s == null -> Row(verticalAlignment = Alignment.CenterVertically) {
                    StatusGlyph(GlyphKind.Spinner)
                    Spacer(Modifier.width(10.dp))
                    ZText("Starting the sign-in on the device…", ZType.sans(15f), c.secondary)
                }
                s.mode == Agents.LoginMode.PasteCode -> {
                    ZText("Finish signing in in the browser, then paste the code it shows here.", ZType.sans(15f), c.secondary)
                    Spacer(Modifier.height(12.dp))
                    ZTextField(code, { code = it }, "Paste the code", mono = true, onSubmit = {})
                }
                else -> Row(verticalAlignment = Alignment.CenterVertically) {
                    StatusGlyph(GlyphKind.Spinner)
                    Spacer(Modifier.width(10.dp))
                    ZText("Waiting for the browser to finish…", ZType.sans(15f), c.secondary)
                }
            }
            if (url.isNotEmpty()) {
                Spacer(Modifier.height(14.dp))
                Box(
                    Modifier
                        .fillMaxWidth()
                        .background(c.controlFill, RoundedCornerShape(12.dp))
                        .padding(12.dp),
                ) { ZText(url, ZType.mono(12f), c.secondary, maxLines = 3) }
                Spacer(Modifier.height(10.dp))
                Row {
                    CapsuleButton("Open Page", Modifier.weight(1f), style = ButtonStyle.Secondary, glyph = Glyph.ArrowUpRight, compact = true) { openUrl(context, url) }
                    Spacer(Modifier.width(10.dp))
                    CapsuleButton("Copy Link", Modifier.weight(1f), style = ButtonStyle.Secondary, glyph = Glyph.Link, compact = true) {
                        copy(context, url)
                        overlays.toast("Link copied")
                    }
                }
            }
            Spacer(Modifier.height(16.dp))
            if (s?.mode == Agents.LoginMode.PasteCode && failure == null) {
                CapsuleButton("Finish Sign-In", Modifier.fillMaxWidth(), enabled = code.isNotBlank(), busy = busy) {
                    busy = true
                    scope.launch {
                        runCatching { model.hostCall(device, Agents.COMPLETE_LOGIN, JSONObject().put("loginId", s.loginId).put("code", code.trim())) }
                            .onSuccess { complete() }
                            .onFailure { failure = it.userMessage() }
                        busy = false
                    }
                }
                Spacer(Modifier.height(8.dp))
            }
            CapsuleButton(if (failure != null) "Close" else "Cancel", Modifier.fillMaxWidth(), style = ButtonStyle.Ghost, compact = true) { dismiss() }
        }
    }
}
