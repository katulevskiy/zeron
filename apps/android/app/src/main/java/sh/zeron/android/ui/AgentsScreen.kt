package sh.zeron.android.ui

import android.content.Context
import android.net.Uri
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject
import sh.zeron.android.core.Agents
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.userMessage
import sh.zeron.android.design.GeistMono
import sh.zeron.android.design.HarnessMark
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.CoreException

/**
 * Settings → Coding agents: the harnesses on an engine device with their
 * install state (Install / progress / Cancel) and agent accounts (browser or
 * paste-code sign-in, sign out). Everything goes over `host_call` to that
 * device's engine — this phone's, or a computer's in account mode; Demo
 * answers from a simulated engine.
 */
@Composable
fun AgentsScreen(model: AppModel, onBack: () -> Unit) {
    val client by model.client.collectAsState()
    val workspace by model.workspace.collectAsState()
    val devices = remember(client, workspace?.devices) { model.executionDevices() }
    var deviceId by remember { mutableStateOf<String?>(null) }
    // This phone first: it's the device whose agents are managed from here most.
    val device = devices.firstOrNull { it.id == deviceId } ?: devices.firstOrNull { it.isSelf }
        ?: devices.firstOrNull { it.online } ?: devices.firstOrNull()
    var harnesses by remember { mutableStateOf<List<Agents.Harness>?>(null) }
    var accounts by remember { mutableStateOf(Agents.Accounts(emptyList(), emptyMap())) }
    var versions by remember { mutableStateOf<Map<String, Agents.Version>>(emptyMap()) }
    var error by remember { mutableStateOf<String?>(null) }
    val installs = remember { mutableStateMapOf<String, Job>() }
    var reloads by remember { mutableIntStateOf(0) }
    var signingIn by remember { mutableStateOf<Agents.Harness?>(null) }
    var signingOut by remember { mutableStateOf<Pair<Agents.Harness, Agents.Account>?>(null) }
    var pickDevice by remember { mutableStateOf(false) }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()

    LaunchedEffect(device?.id, reloads, client) {
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
        installs[h.id] = scope.launch {
            try {
                harnesses = Agents.harnesses(model.hostCall(d.id, Agents.INSTALL_HARNESS, JSONObject().put("harness", h.id)))
                snackbar.showSnackbar("${h.name} installed")
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                if (e is CoreException.HostUnavailable && Agents.isTimeout(e.reason)) {
                    // Still installing on the device: watch the catalog instead.
                    snackbar.showSnackbar(if (awaitInstalled(model, d.id, h.id)) "${h.name} installed" else "${h.name} didn't finish installing. Check the engine log.")
                } else {
                    val msg = e.userMessage()
                    if (!msg.contains("cancelled", ignoreCase = true)) snackbar.showSnackbar("Couldn't install ${h.name}: $msg")
                }
            } finally {
                installs.remove(h.id)
                reloads++
            }
        }
    }

    fun cancelInstall(h: Agents.Harness) {
        val d = device ?: return
        scope.launch { runCatching { model.hostCall(d.id, Agents.CANCEL_INSTALL, JSONObject().put("harness", h.id)) } }
    }

    SubPage(
        title = "Coding agents",
        subtitle = device?.name,
        onBack = onBack,
        overlay = { SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter).navigationBarsPadding()) },
    ) {
        if (device == null) {
            item { EmptyNote(ZIcons.Bot, "No engine to manage", "Agents install on a device running Zeron — this phone's engine or one of your computers.") }
            return@SubPage
        }
        if (devices.size > 1) {
            item {
                Row(Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) {
                    ContextChip(device.name, leading = { ZIcon(sh.zeron.android.core.DeviceIdentity.icon(device.platform), null, Modifier.size(16.dp)) }, onClick = { pickDevice = true }) {
                        ChoiceMenu(pickDevice, { pickDevice = false }, listOf(MenuSection("Device", devices.map { d ->
                            MenuChoice(d.name, d.id == device.id, if (d.isSelf) "This phone" else if (d.online) "Online" else "Offline") {
                                deviceId = d.id
                                harnesses = null
                            }
                        })))
                    }
                }
            }
        }
        val list = harnesses
        when {
            list == null -> item {
                Box(Modifier.fillMaxWidth().padding(top = 64.dp), contentAlignment = Alignment.Center) { LoadingIndicator() }
            }
            list.isEmpty() -> item {
                EmptyNote(ZIcons.Warning, "Couldn't load the agents", error ?: "The device didn't answer.") {
                    FilledTonalButton(onClick = { reloads++ }, shapes = ButtonDefaults.shapes()) { Text("Try again") }
                }
            }
            else -> {
                // Installed first; the order within each group is the engine's.
                val sorted = list.sortedByDescending { it.installed }
                for (h in sorted) {
                    item(h.id) {
                        HarnessCard(
                            h,
                            accounts.forHarness(h.id),
                            accounts.warnings[h.id],
                            versions[h.id],
                            installing = h.id in installs,
                            onInstall = { install(h) },
                            onCancel = { cancelInstall(h) },
                            onSignIn = { signingIn = h },
                            onSignOut = { signingOut = h to it },
                        )
                    }
                }
            }
        }
    }

    signingIn?.let { h ->
        SignInSheet(model, device!!.id, h, scope, onDismiss = { signingIn = null }) {
            signingIn = null
            reloads++
            scope.launch { snackbar.showSnackbar("Signed in to ${h.name}") }
        }
    }
    signingOut?.let { (h, a) ->
        AlertDialog(
            onDismissRequest = { signingOut = null },
            icon = { ZIcon(ZIcons.Logout, null) },
            title = { Text("Sign out of ${a.title}?") },
            text = { Text("${h.name} on ${device?.name ?: "this device"} forgets this account.") },
            confirmButton = {
                TextButton(onClick = {
                    signingOut = null
                    val d = device ?: return@TextButton
                    scope.launch {
                        runCatching {
                            accounts = Agents.accounts(model.hostCall(d.id, Agents.FORGET_ACCOUNT, JSONObject().put("harness", h.id).put("accountId", a.id)))
                        }.onFailure { snackbar.showSnackbar("Couldn't sign out: ${it.userMessage()}") }
                    }
                }) { Text("Sign out", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { signingOut = null }) { Text("Cancel") } },
        )
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
    installing: Boolean,
    onInstall: () -> Unit,
    onCancel: () -> Unit,
    onSignIn: () -> Unit,
    onSignOut: (Agents.Account) -> Unit,
) {
    Surface(
        shape = RoundedCornerShape(28.dp),
        color = cardColor(),
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
    ) {
        Column(Modifier.animateContentSize().padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(
                    Modifier.size(44.dp).clip(RoundedCornerShape(14.dp)).background(MaterialTheme.colorScheme.secondaryContainer),
                    contentAlignment = Alignment.Center,
                ) { HarnessMark(h.id, 24.dp, tint = MaterialTheme.colorScheme.onSecondaryContainer) }
                Spacer(Modifier.width(14.dp))
                Column(Modifier.weight(1f)) {
                    Text(h.name, style = MaterialTheme.typography.titleMedium, maxLines = 1)
                    Text(
                        when {
                            installing -> "Installing…"
                            h.installed -> listOfNotNull("Installed", version?.installed?.let { "v$it" }).joinToString(" · ")
                            h.canInstall -> "Not installed"
                            else -> "Not installed · install it on the device"
                        },
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                    )
                }
                when {
                    installing -> OutlinedButton(onClick = onCancel, shapes = ButtonDefaults.shapes()) { Text("Cancel") }
                    !h.installed && h.canInstall -> Button(onClick = onInstall, shapes = ButtonDefaults.shapes()) { Text("Install") }
                }
            }
            if (installing) {
                Spacer(Modifier.height(14.dp))
                LinearWavyProgressIndicator(Modifier.fillMaxWidth())
            }
            if (h.installed && !installing) {
                for (a in accounts) {
                    Spacer(Modifier.height(10.dp))
                    Surface(shape = RoundedCornerShape(18.dp), color = MaterialTheme.colorScheme.surfaceContainerHigh) {
                        Row(Modifier.padding(start = 14.dp, end = 4.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                            ZIcon(ZIcons.Key, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                            Spacer(Modifier.width(12.dp))
                            Column(Modifier.weight(1f)) {
                                Text(a.title, style = MaterialTheme.typography.bodyLargeEmphasized, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                Text(
                                    listOfNotNull(a.plan, if (a.active) "Active" else null).joinToString(" · ").ifEmpty { "Signed in" },
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    maxLines = 1,
                                )
                            }
                            TextButton(onClick = { onSignOut(a) }) { Text("Sign out") }
                        }
                    }
                }
                if (warning != null) {
                    Spacer(Modifier.height(8.dp))
                    Text(warning, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                }
                Spacer(Modifier.height(12.dp))
                FilledTonalButton(onClick = onSignIn, modifier = Modifier.fillMaxWidth(), shapes = ButtonDefaults.shapes()) {
                    ZIcon(if (accounts.isEmpty()) ZIcons.Key else ZIcons.Plus, null, Modifier.size(ButtonDefaults.IconSize))
                    Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                    Text(if (accounts.isEmpty()) "Sign in" else "Add account")
                }
            }
        }
    }
}

/**
 * An agent sign-in: open the page the engine hands back in a Custom Tab;
 * browser logins finish on their own (poll), paste-code logins take the
 * code here. Closing the sheet early cancels the login on the device.
 */
@Composable
private fun SignInSheet(model: AppModel, device: String, h: Agents.Harness, scope: CoroutineScope, onDismiss: () -> Unit, onDone: () -> Unit) {
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current
    val sheet = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var start by remember { mutableStateOf<Agents.LoginStart?>(null) }
    var url by remember { mutableStateOf("") }
    var failure by remember { mutableStateOf<String?>(null) }
    var code by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var finished by remember { mutableStateOf(false) }

    fun complete() {
        finished = true
        onDone()
    }

    LaunchedEffect(Unit) {
        val s = runCatching { Agents.loginStart(model.hostCall(device, Agents.START_LOGIN, JSONObject().put("harness", h.id))) }
            .onFailure { failure = it.userMessage() }.getOrNull()
        if (s == null) {
            if (failure == null) failure = "The device didn't start a sign-in."
            return@LaunchedEffect
        }
        start = s
        url = s.url
        if (url.isNotEmpty()) openPage(context, model, url)
        if (s.mode == Agents.LoginMode.Browser) {
            while (!finished) {
                delay(2000)
                val poll = runCatching { Agents.loginPoll(model.hostCall(device, Agents.POLL_LOGIN, JSONObject().put("loginId", s.loginId))) }
                    .getOrElse { Agents.LoginPoll.Pending(null) }
                when (poll) {
                    Agents.LoginPoll.Done -> return@LaunchedEffect complete()
                    is Agents.LoginPoll.Failed -> {
                        failure = poll.message
                        return@LaunchedEffect
                    }
                    is Agents.LoginPoll.Pending -> if (poll.url != null && url.isEmpty()) {
                        url = poll.url
                        openPage(context, model, poll.url)
                    }
                }
            }
        }
    }
    DisposableEffect(Unit) {
        onDispose {
            val s = start
            if (!finished && s != null) scope.launch { runCatching { model.hostCall(device, Agents.CANCEL_LOGIN, JSONObject().put("loginId", s.loginId)) } }
        }
    }

    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheet, containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
        Column(Modifier.padding(horizontal = 24.dp).padding(bottom = 24.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                HarnessMark(h.id, 28.dp, tint = MaterialTheme.colorScheme.onSurface)
                Spacer(Modifier.width(12.dp))
                Text("Sign in to ${h.name}", style = MaterialTheme.typography.headlineSmallEmphasized)
            }
            Spacer(Modifier.height(16.dp))
            val s = start
            when {
                failure != null -> Text(failure!!, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.error)
                s == null -> Waiting("Starting the sign-in on the device…")
                s.mode == Agents.LoginMode.PasteCode -> {
                    Text("Finish signing in in the browser, then paste the code it shows here.", style = MaterialTheme.typography.bodyLarge)
                    Spacer(Modifier.height(12.dp))
                    OutlinedTextField(
                        code,
                        { code = it },
                        placeholder = { Text("Paste the code") },
                        singleLine = true,
                        shape = RoundedCornerShape(16.dp),
                        textStyle = MaterialTheme.typography.bodyLarge.copy(fontFamily = GeistMono),
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                else -> Waiting("Waiting for the browser to finish…")
            }
            if (url.isNotEmpty()) {
                Spacer(Modifier.height(16.dp))
                Surface(shape = RoundedCornerShape(16.dp), color = MaterialTheme.colorScheme.surfaceContainerHighest, modifier = Modifier.fillMaxWidth()) {
                    Text(url, style = MaterialTheme.typography.bodySmall, fontFamily = GeistMono, maxLines = 3, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(12.dp))
                }
                Spacer(Modifier.height(10.dp))
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    FilledTonalButton(onClick = { openPage(context, model, url, force = true) }, modifier = Modifier.weight(1f), shapes = ButtonDefaults.shapes()) {
                        ZIcon(ZIcons.Link, null, Modifier.size(ButtonDefaults.IconSize))
                        Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                        Text("Open page")
                    }
                    FilledTonalButton(onClick = { clipboard.setText(AnnotatedString(url)) }, modifier = Modifier.weight(1f), shapes = ButtonDefaults.shapes()) {
                        ZIcon(ZIcons.Copy, null, Modifier.size(ButtonDefaults.IconSize))
                        Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                        Text("Copy link")
                    }
                }
            }
            Spacer(Modifier.height(20.dp))
            if (s?.mode == Agents.LoginMode.PasteCode && failure == null) {
                Button(
                    onClick = {
                        busy = true
                        scope.launch {
                            runCatching { model.hostCall(device, Agents.COMPLETE_LOGIN, JSONObject().put("loginId", s.loginId).put("code", code.trim())) }
                                .onSuccess { complete() }
                                .onFailure { failure = it.userMessage() }
                            busy = false
                        }
                    },
                    enabled = code.isNotBlank() && !busy,
                    modifier = Modifier.fillMaxWidth(),
                    shapes = ButtonDefaults.shapes(),
                ) { Text("Finish sign-in") }
                Spacer(Modifier.height(8.dp))
            }
            TextButton(onClick = onDismiss, modifier = Modifier.fillMaxWidth()) { Text(if (failure != null) "Close" else "Cancel") }
        }
    }
}

@Composable
private fun Waiting(text: String) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        LoadingIndicator(Modifier.size(32.dp))
        Spacer(Modifier.width(8.dp))
        Text(text, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** Demo sign-in pages are placeholders: nothing to open unless asked. */
private fun openPage(context: Context, model: AppModel, url: String, force: Boolean = false) {
    if (model.isDemo && !force) return
    runCatching { CustomTabsIntent.Builder().setShowTitle(true).build().launchUrl(context, Uri.parse(url)) }
}

@Composable
private fun EmptyNote(icon: Int, title: String, detail: String, action: @Composable () -> Unit = {}) {
    Column(Modifier.fillMaxWidth().padding(horizontal = 32.dp, vertical = 48.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        ZIcon(icon, null, Modifier.size(40.dp), tint = MaterialTheme.colorScheme.outline)
        Spacer(Modifier.height(12.dp))
        Text(title, style = MaterialTheme.typography.titleLarge)
        Spacer(Modifier.height(6.dp))
        Text(detail, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = androidx.compose.ui.text.style.TextAlign.Center)
        Spacer(Modifier.height(16.dp))
        action()
    }
}
