package sh.zeron.android.voice

/** Selection never silently moves a call to another device. */
data class VoiceHost(val id: String, val name: String, val online: Boolean, val self: Boolean)

fun startHost(hosts: List<VoiceHost>, selected: String?): VoiceHost? {
    if (selected != null) return hosts.firstOrNull { it.id == selected && it.online }
    return hosts.filter { it.online }.singleOrNull()
}

/** Invalidates all callbacks before native capture or remote ownership is released. */
class VoiceGeneration {
    private var generation = 0L
    var live = false
        private set
    fun begin(): Long { check(!live); live = true; return ++generation }
    fun accepts(id: Long) = live && id == generation
    fun end(): Boolean {
        if (!live) return false
        live = false
        ++generation
        return true
    }
}

/** A late unmute acknowledgement cannot undo a newer local mute. */
class VoiceMute {
    var requested = false
        private set
    fun request(muted: Boolean) { requested = muted }
    fun effective(acknowledged: Boolean) = requested || acknowledged
}

fun voiceElapsed(seconds: Long): String {
    val value = seconds.coerceAtLeast(0)
    return if (value < 3600) "%d:%02d".format(value / 60, value % 60)
    else "%d:%02d:%02d".format(value / 3600, value / 60 % 60, value % 60)
}
