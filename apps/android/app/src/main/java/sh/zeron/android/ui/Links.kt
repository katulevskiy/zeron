package sh.zeron.android.ui

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.util.Log
import androidx.browser.customtabs.CustomTabColorSchemeParams
import androidx.browser.customtabs.CustomTabsIntent
import sh.zeron.android.core.AppModel

/**
 * Open a web page in a Custom Tab (keeps the user in Zeron's task — sign-ins
 * come back through the `zeron://` intent filter), falling back to any
 * browser.
 */
fun openUrl(context: Context, url: String, toolbarColor: Int? = null) {
    val uri = Uri.parse(url)
    try {
        val builder = CustomTabsIntent.Builder().setShowTitle(true)
        if (toolbarColor != null) builder.setDefaultColorSchemeParams(CustomTabColorSchemeParams.Builder().setToolbarColor(toolbarColor).build())
        val intent = builder.build()
        if (context !is android.app.Activity) intent.intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        intent.launchUrl(context, uri)
    } catch (e: ActivityNotFoundException) {
        try {
            context.startActivity(Intent(Intent.ACTION_VIEW, uri).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        } catch (e2: ActivityNotFoundException) {
            Log.w(AppModel.TAG, "no browser for $url", e2)
        }
    }
}
