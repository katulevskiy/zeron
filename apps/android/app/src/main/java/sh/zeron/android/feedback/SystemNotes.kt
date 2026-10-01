package sh.zeron.android.feedback

/** What the system is doing to the app's feedback, as one line for the Settings page. */
data class SystemNote(val text: String, val kind: Kind = Kind.Info) {
    enum class Kind {
        Info,

        /** The phone's "Touch sounds" setting is off: tappable, opens the explanation and the override. */
        TouchSounds,
    }
}

object SystemNotes {
    /** Notes for the Settings page, most important first; empty when the system is not in the way. */
    fun build(env: FeedbackEnvironment, settings: FeedbackSettings): List<SystemNote> = buildList {
        if (!env.hasVibrator) add(SystemNote("This device has no vibration motor, so haptics are off."))
        else if (!env.systemTouchHaptics) add(SystemNote("Touch vibration is turned off in system settings, so haptics are off."))
        if (!env.ringerNormal) add(SystemNote("The phone is on silent or vibrate, so sounds are muted."))
        else if (env.silencedByDnd) add(SystemNote("Do Not Disturb is silencing sounds."))
        else if (!env.streamAudible) add(SystemNote("The system sound volume is at zero."))
        else if (!env.systemTouchSounds && settings.sounds && settings.interfaceSounds) {
            add(
                SystemNote(
                    if (settings.ignoreSystemTouchSounds) "Touch sounds are off in your phone's settings, but Zeron is playing its interface sounds anyway. Tap for details."
                    else "Touch sounds are off in your phone's settings, so Zeron's interface sounds are muted too. Tap to fix.",
                    SystemNote.Kind.TouchSounds,
                ),
            )
        }
    }
}
