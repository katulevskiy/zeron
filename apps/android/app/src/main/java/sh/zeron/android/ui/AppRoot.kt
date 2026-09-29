package sh.zeron.android.ui

import androidx.activity.ComponentActivity
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import sh.zeron.android.core.AppMode
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.LocalColors
import sh.zeron.android.design.LocalOverlays
import sh.zeron.android.design.OverlayHost
import sh.zeron.android.ui.home.HomeScreen
import sh.zeron.android.ui.onboarding.OnboardingScreen
import sh.zeron.android.ui.onboarding.PhoneSetupScreen
import sh.zeron.android.ui.onboarding.SignInScreen
import sh.zeron.android.ui.project.NewProjectScreen
import sh.zeron.android.ui.project.ProjectScreen
import sh.zeron.android.ui.session.NewSessionScreen
import sh.zeron.android.ui.session.SessionScreen
import sh.zeron.android.ui.sessions.FolderScreen
import sh.zeron.android.ui.sessions.SearchScreen
import sh.zeron.android.ui.settings.AgentsScreen
import sh.zeron.android.ui.settings.RuntimeScreen

val LocalModel = staticCompositionLocalOf<AppModel> { error("no model") }
val LocalActivity = staticCompositionLocalOf<ComponentActivity> { error("no activity") }

/**
 * Root: first-run mode choice, the sign-in / phone-setup gates, then the
 * app's back stack — with the overlay layer (menus, sheets, toasts) above
 * everything.
 */
@Composable
fun AppRoot(model: AppModel, activity: ComponentActivity) {
    val mode by model.mode.collectAsState()
    val client by model.client.collectAsState()
    val overlays = LocalOverlays.current
    val nav = remember { Navigator() }
    val colors = LocalColors.current

    LaunchedEffect(Unit) { model.openRequests.collect { nav.openSession(it) } }
    // A new client (mode switch, sign-in) starts from the front page.
    // A menu or sheet belongs to the screen that opened it.
    LaunchedEffect(nav.depth) { overlays.dismissAll() }
    LaunchedEffect(client) {
        nav.popToRoot()
        overlays.dismissAll()
    }

    CompositionLocalProvider(LocalModel provides model, LocalNavigator provides nav, LocalActivity provides activity) {
        Box(
            Modifier
                .fillMaxSize()
                .background(colors.background),
        ) {
            when {
                mode == null -> OnboardingScreen()
                mode == AppMode.Account && client == null -> SignInScreen()
                mode == AppMode.Phone && client == null -> PhoneSetupScreen()
                else -> StackHost(nav) { route -> Screen(route) }
            }
            OverlayHost(overlays)
        }
    }
}

@Composable
private fun Screen(route: Route) {
    when (route) {
        Route.Home -> HomeScreen()
        is Route.Session -> SessionScreen(route.chatId)
        is Route.Folder -> FolderScreen(route.id, route.name)
        is Route.Project -> ProjectScreen(route.id)
        is Route.NewSession -> NewSessionScreen(route.projectId)
        is Route.NewProject -> NewProjectScreen(route.forNewSession)
        Route.Search -> SearchScreen()
        Route.Runtime -> RuntimeScreen()
        Route.Agents -> AgentsScreen()
    }
}
