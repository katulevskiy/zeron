package sh.zeron.android.voice

/** Safe, stage-specific failures; never expose provider payloads or image data. */
internal class VoicePhotoException(val reason: Reason) : Exception(reason.message) {
    enum class Reason(val message: String) {
        ENDED("The call ended before the photo was added. Start a new call."),
        READ("Couldn't read the camera photo. Take another picture."),
        BUSY("A photo is already being added. Please wait."),
        SIZE("The photo is too large for this connection. Take another picture."),
        TRANSPORT("Couldn't send the photo over the voice connection. Try again."),
        REJECTED("Jarvis couldn't accept this photo. Try a new call."),
    }
}
