package sh.zeron.android.ui.settings

import android.content.Intent
import android.provider.Settings
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.layout.windowInsetsTopHeight
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import sh.zeron.android.BuildConfig
import sh.zeron.android.core.AppMode
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.SessionLists
import sh.zeron.android.design.Anchor
import sh.zeron.android.design.Appearance
import sh.zeron.android.design.ButtonStyle
import sh.zeron.android.design.CapsuleButton
import sh.zeron.android.design.DialogCard
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.GlyphIcon
import sh.zeron.android.design.Group
import sh.zeron.android.design.Hairline
import sh.zeron.android.design.LargeTitle
import sh.zeron.android.design.ListRow
import sh.zeron.android.design.LocalColors
import sh.zeron.android.design.LocalOverlays
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.Overlays
import sh.zeron.android.design.Toggle
import sh.zeron.android.design.ZText
import sh.zeron.android.design.ZType
import sh.zeron.android.design.anchor
import sh.zeron.android.design.rememberAnchor
import sh.zeron.android.ui.LocalActivity
import sh.zeron.android.ui.LocalModel
import sh.zeron.android.ui.LocalNavigator
import sh.zeron.android.ui.Route
import sh.zeron.runtime.RuntimePermissions
import uniffi.zeron_core.coreVersion

fun AppMode.title() = when (this) {
    AppMode.Phone -> "This phone"
    AppMode.Account -> "Zeron account"
    AppMode.Demo -> "Demo"
}

fun AppMode.detail() = when (this) {
    AppMode.Phone -> "Agents run on this device, no computer needed"
    AppMode.Account -> "Drive agents on your computers and servers"
    AppMode.Demo -> "Explore with a sample workspace, offline"
}

fun AppMode.glyph() = when (this) {
    AppMode.Phone -> Glyph.Phone
    AppMode.Account -> Glyph.Person
    AppMode.Demo -> Glyph.Sparkle
}

/** Settings: mode, account, engine & agents, devices, notifications, appearance, about. */
@Composable
fun SettingsTab(bottomPadding: Dp) {
    val model = LocalModel.current
    val nav = LocalNavigator.current
    val overlays = LocalOverlays.current
    val c = LocalColors.current
    val context = LocalContext.current
    val activity = LocalActivity.current
    val mode by model.mode.collectAsState()
    val ws by model.workspace.collectAsState()
    val appearance by model.appearance.collectAsState()
    val runtime by model.phone.state.collectAsState()
    // Permission state isn't observable: re-read when we come back to the foreground.
    var resumes by remember { mutableIntStateOf(0) }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { resumes++ }
    val permitted = remember(resumes) { model.notifier.permitted }
    var notifications by remember { mutableStateOf(model.prefs.notifications) }
    val modeAnchor = rememberAnchor()

    Box(Modifier.fillMaxSize()) {
        Column(
            Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .windowInsetsPadding(WindowInsets.statusBars),
        ) {
            LargeTitle("Settings")
            val m = mode ?: AppMode.Demo
            Group("Mode", footer = "Switching keeps each mode's data; the phone's engine keeps running until you stop it.") {
                ListRow(m.title(), Modifier.anchor(modeAnchor), subtitle = model.accountDetail, glyph = m.glyph(), tint = c.accent, trailing = {
                    ZText("Switch", ZType.sans(15f), c.accent)
                }) { switchMode(model, overlays, modeAnchor, m) }
            }
            if (m == AppMode.Account) {
                Group("Account") {
                    ListRow(model.accountName, subtitle = model.accountDetail, glyph = Glyph.Person)
                    Hairline()
                    ListRow("Sign Out", glyph = Glyph.SignOut, destructive = true) {
                        overlays.confirm("Sign out?", "This phone forgets the account's local copy. Your sessions stay on your devices.", "Sign Out", destructive = true) { model.signOut() }
                    }
                }
            }
            Group("Agents") {
                ListRow("On-device engine", subtitle = model.phone.stateLabel(runtime) + if (m == AppMode.Phone) "" else " · used in This phone mode", glyph = Glyph.Chip, chevron = true) { nav.push(Route.Runtime) }
                Hairline()
                ListRow("Coding agents", subtitle = "Install agents and sign in to their accounts", glyph = Glyph.Terminal, chevron = true) { nav.push(Route.Agents) }
            }
            val devices = ws?.devices?.filter { it.isExecutionHost } ?: emptyList()
            if (devices.isNotEmpty()) {
                Group("Devices") {
                    devices.forEachIndexed { i, d ->
                        if (i > 0) Hairline()
                        ListRow(d.name, subtitle = listOfNotNull(if (d.online) "Online" else "Offline", d.version?.let { "v$it" }, "${d.sessionCount} sessions").joinToString(" · "), glyph = if (d.platform == "android") Glyph.Phone else Glyph.Desktop, trailing = {
                            Box(Modifier.size(8.dp).background(if (d.online) c.success else c.tertiary, CircleShape))
                        })
                    }
                }
            }
            Group("Notifications", footer = if (m == AppMode.Phone) null else "Local notifications come from this phone's engine. Account sessions notify through your desktop's push settings.") {
                ListRow(
                    "Session notifications",
                    subtitle = if (!permitted) "Not allowed — tap to allow" else "When a session finishes, needs you or fails",
                    glyph = Glyph.Bell,
                    trailing = {
                        Toggle(notifications && permitted) { on ->
                            if (on && !permitted) {
                                if (model.prefs.askedNotifications) {
                                    context.startActivity(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName))
                                } else {
                                    model.prefs.askedNotifications = true
                                    RuntimePermissions.requestNotificationPermission(activity)
                                }
                            }
                            notifications = on
                            model.prefs.notifications = on
                        }
                    },
                )
            }
            Group("Appearance") {
                Appearance.entries.forEachIndexed { i, a ->
                    if (i > 0) Hairline()
                    ListRow(a.name, glyph = when (a) {
                        Appearance.System -> Glyph.HalfCircle
                        Appearance.Light -> Glyph.Sun
                        Appearance.Dark -> Glyph.Moon
                    }, trailing = { if (a == appearance) GlyphIcon(Glyph.Check, c.accent, size = 18.dp) }) { model.setAppearance(a) }
                }
            }
            Group("Sessions") {
                ListRow("Archived Sessions", subtitle = ws?.archived?.size?.let { "$it archived" }, glyph = Glyph.Archive, chevron = true) {
                    nav.push(Route.Folder(SessionLists.ARCHIVED, "Archived"))
                }
            }
            if (m == AppMode.Demo) DeveloperGroup(model, overlays)
            Group("About") {
                ListRow("Zeron ${BuildConfig.VERSION_NAME}", subtitle = "Core ${coreVersion()} · ${android.os.Build.SUPPORTED_ABIS.firstOrNull() ?: ""}", glyph = Glyph.Info)
                Hairline()
                ListRow("Open-source notices", glyph = Glyph.Stack, chevron = true) { notices(overlays) }
            }
            Spacer(Modifier.height(bottomPadding))
        }
        Box(
            Modifier
                .fillMaxWidth()
                .windowInsetsTopHeight(WindowInsets.statusBars)
                .background(Brush.verticalGradient(listOf(c.background, c.background.copy(alpha = 0.85f)))),
        )
    }
}

@Composable
private fun DeveloperGroup(model: AppModel, overlays: Overlays) {
    val scaleAnchor = rememberAnchor()
    var fast by remember { mutableStateOf(model.prefs.demoFast) }
    Group("Demo", footer = "Restarts the demo workspace.") {
        ListRow("Transcript size", Modifier.anchor(scaleAnchor), subtitle = model.prefs.demoScale.replaceFirstChar { it.uppercase() }, glyph = Glyph.Stack, chevron = true) {
            overlays.menu(scaleAnchor, "Flagship transcript", listOf("normal" to "Normal", "big" to "Big · 120 turns", "huge" to "Huge · 600 turns").map { (id, label) ->
                MenuEntry.Item(label, checked = model.prefs.demoScale == id) {
                    model.prefs.demoScale = id
                    model.restartDemo()
                }
            })
        }
        Hairline()
        ListRow("Fast streaming", glyph = Glyph.Sparkle, trailing = {
            Toggle(fast) {
                fast = it
                model.prefs.demoFast = it
                model.restartDemo()
            }
        })
    }
}

private fun switchMode(model: AppModel, overlays: Overlays, anchor: Anchor, current: AppMode) {
    overlays.menu(anchor, "Use Zeron with", AppMode.entries.map { m ->
        MenuEntry.Item(m.title(), subtitle = m.detail(), glyph = m.glyph(), checked = m == current, enabled = m != AppMode.Phone || model.phone.isSupportedAbi) {
            if (m != current) model.chooseMode(m)
        }
    })
}

private fun notices(overlays: Overlays) = overlays.dialog { dismiss ->
    val c = LocalColors.current
    DialogCard("Open-source notices", null) {
        for ((name, licence) in listOf(
            "PRoot (on-device engine)" to "GPL-2.0 · github.com/termux/proot",
            "talloc" to "LGPL-3.0 · talloc.samba.org",
            "Alpine Linux minirootfs" to "Various · alpinelinux.org",
            "Geist, Geist Mono" to "SIL Open Font License 1.1",
            "UniFFI, JNA" to "MPL-2.0 · Apache-2.0 / LGPL-2.1",
        )) {
            ZText(name, ZType.sans(15f, androidx.compose.ui.text.font.FontWeight.Medium), c.text)
            ZText(licence, ZType.sans(13f), c.secondary)
            Spacer(Modifier.height(10.dp))
        }
        Spacer(Modifier.height(6.dp))
        CapsuleButton("Done", Modifier.fillMaxWidth(), style = ButtonStyle.Secondary, compact = true) { dismiss() }
    }
}
