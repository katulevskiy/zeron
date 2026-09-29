package sh.zeron.android.core

import android.content.Context
import android.content.SharedPreferences
import org.json.JSONObject
import sh.zeron.android.design.Appearance
import java.util.UUID

/** Which engine the app drives (chosen on first run, switchable in Settings). */
enum class AppMode {
    /** The on-device engine (the `:runtime` guest) over its local edge. */
    Phone,

    /** Remote control of an account's desktops/servers via the production edge. */
    Account,

    /** Offline dataset with a simulated host (also the development harness). */
    Demo,
}

/** Non-secret app preferences. Secrets live in [SecureStore]. */
class Prefs(context: Context) {
    private val p: SharedPreferences = context.getSharedPreferences("zeron", Context.MODE_PRIVATE)

    var mode: AppMode?
        get() = p.getString("mode", null)?.let { runCatching { AppMode.valueOf(it) }.getOrNull() }
        set(v) = p.edit().putString("mode", v?.name).apply()

    var appearance: Appearance
        get() = runCatching { Appearance.valueOf(p.getString("appearance", "System")!!) }.getOrDefault(Appearance.System)
        set(v) = p.edit().putString("appearance", v.name).apply()

    /** Stable per-install device id (`android-xxxxxxxx`), stamped on every command. */
    val deviceId: String
        get() = p.getString("deviceId", null) ?: ("android-" + UUID.randomUUID().toString().take(8)).also {
            p.edit().putString("deviceId", it).apply()
        }

    /** Demo transcript size: normal / big / huge (Settings → Developer, or `--es demo huge`). */
    var demoScale: String
        get() = p.getString("demoScale", "normal")!!
        set(v) = p.edit().putString("demoScale", v).apply()

    var demoFast: Boolean
        get() = p.getBoolean("demoFast", false)
        set(v) = p.edit().putBoolean("demoFast", v).apply()

    var notifications: Boolean
        get() = p.getBoolean("notifications", true)
        set(v) = p.edit().putBoolean("notifications", v).apply()

    /** The notification permission was asked for once already (never nag). */
    var askedNotifications: Boolean
        get() = p.getBoolean("askedNotifications", false)
        set(v) = p.edit().putBoolean("askedNotifications", v).apply()

    /** The user started the phone's engine before: bring it back up on launch. */
    var phoneAutostart: Boolean
        get() = p.getBoolean("phoneAutostart", false)
        set(v) = p.edit().putBoolean("phoneAutostart", v).apply()

    var collapsedSections: Set<String>
        get() = p.getStringSet("collapsedSections", emptySet())!!
        set(v) = p.edit().putStringSet("collapsedSections", v).apply()

    /** OAuth `state` of the sign-in in flight (survives the browser round trip). */
    var pendingAuthState: String?
        get() = p.getString("authState", null)
        set(v) = p.edit().putString("authState", v).apply()

    /** Unsent composer text per chat. */
    fun draft(chatId: String): String = drafts().optString(chatId, "")

    fun saveDraft(chatId: String, text: String) {
        val all = drafts()
        if (text.isBlank()) all.remove(chatId) else all.put(chatId, text)
        p.edit().putString("drafts", all.toString()).apply()
    }

    private fun drafts() = runCatching { JSONObject(p.getString("drafts", "{}")!!) }.getOrDefault(JSONObject())

    /** The new-session page's picks (project/host/harness/model/effort). */
    var newSessionDraft: String?
        get() = p.getString("newSessionDraft", null)
        set(v) = p.edit().putString("newSessionDraft", v).apply()
}
