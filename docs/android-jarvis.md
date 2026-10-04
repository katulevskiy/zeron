# Jarvis on Android

This branch layers the desktop voice integration on Android PR #700 at
`2c67ac41c569bb2055158ab8a12ed69ea748c33f`. It is a fork-only trial branch;
there is no upstream Jarvis PR yet.

## Try it

Install the debug APK as an update to Zeron. In Settings → Coding agents,
install Codex and sign in with ChatGPT on this phone. In Settings → Jarvis,
choose this phone and a voice, then tap **Start Jarvis**. You can also tap
**Talk with Jarvis** on the Sessions page. The first call explains audio and
tool access, then asks Android for microphone permission.

Alternatively choose an online desktop advertising `voice-client-media-v1`.
That desktop must run the integrated voice branch with
`ZERON_REMOTE_VOICE=1`, have Codex signed in with ChatGPT, and belong to the
same Zeron account/workspace as the phone. The ordinary installed desktop
release does not necessarily have this capability. The phone engine enables
remote voice in its own guest environment; it needs no desktop helper binary.

Audio goes directly between the Android native WebRTC endpoint and OpenAI.
The selected execution device owns Codex, authentication, tools, and the
canonical conversation. **Open conversation** lets you answer tool approvals
or questions there. Changing execution device is disabled during a call;
changing voice applies to the next call. A missing selected device never
silently falls back to another device.

Mute stops local capture immediately and updates the host. Unmute waits for
the current owner. Hang-up closes audio before releasing remote ownership.
Navigation and screen locking keep an explicitly started call alive in a
microphone foreground service. The ongoing notification provides mute and
hang-up controls (enable notifications if Android has blocked them). Removing
the app task, losing audio focus/network/ownership, signing out, or restarting
the engine ends the call. Reconnection always requires starting a new call.

On Android 12+, connected communication headsets take precedence; otherwise
the speaker button switches the built-in speaker/earpiece. Proximity selects
the earpiece and turns the screen off near your face when supported. Older
Android uses the existing system headset route. No wake word or always-on
microphone is used.

## Build and verify

Use Java 21, Android SDK 37, NDK 29.0.14206865, cargo-ndk, cargo-zigbuild,
Zig, and Rust's Android and Linux-musl targets for both arm64 and x86_64.

```sh
export ANDROID_HOME="$HOME/Android/Sdk"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/29.0.14206865"
bash scripts/android/build-core.sh
bash scripts/android/build-engine.sh
cd apps/android
./gradlew :app:assembleDebug :app:testDebugUnitTest :app:lintDebug -PzeronSkipCore
./gradlew :app:connectedDebugAndroidTest -PzeronSkipCore
```

`-PzeronSkipCore` reuses the explicitly rebuilt native core. Runtime scripts
only rebuild missing outputs, so rerun `build-engine.sh` explicitly after
engine changes. The APK includes both ABIs and its pinned rootfs/proot payload.

`VoicePolicyTest` covers host selection, stale generations and mute ordering.
`WebRtcVoicePeerTest` negotiates a real local JNI/ICE/data-channel connection,
verifies negotiation does not capture, exercises mute/unmute and repeated
close/restart, and checks recording and communication-mode cleanup. It does
not call OpenAI or require credentials. Shared Rust suites cover permissions,
pending media cancellation, host eligibility, ownership loss, two-engine
remote calls, and preservation of #700's mobile subagent/callback behavior.

Debug-only `route=jarvis-preview` renders the idle stage without a call or
microphone. Real phone acoustic quality, Bluetooth headset behavior, and a
signed-in Android-to-OpenAI conversation still require device testing.

## Validation on this Linux host

- Rust mobile/core/orb/protocol/session suites: 118 passed, one unrelated
  ignored benchmark; desktop `cargo check -p zeron` passed.
- Engine voice: 16 default tests and all four opt-in remote tests passed,
  including two-engine routing and owner-loss cleanup.
- PR #700 mobile subagent/callback regression suite: all three passed.
- Workspace file suite: all eight passed, including atomic move validation.
- Android JVM suite: all 272 passed.
- ARM64 and x86_64 native cores and static guest engines rebuilt from this
  branch and included in the APK.
- Android lint: 12 pre-existing errors outside the voice implementation
  (haptic API guards, generated Cleaner detection, and older Compose/navigation
  patterns); no Jarvis lint errors. These are not suppressed by this branch.
- Android 15 x86_64 emulator: both real JNI audio/routing tests passed. The
  microphone remained inactive during negotiation; mute/close released the
  active recording, and a second call successfully reopened and closed it.
- On-device guest engine booted healthy, advertised this phone as a voice
  host, and reached the native call coordinator. Missing Codex ended cleanly
  and removed the microphone service. Denying the platform microphone prompt
  showed the microphone-specific recovery message without starting that service.
- Idle stage checked in light mode and dark mode with 200% system font scale;
  voice/device settings and the shared animated geometry rendered correctly.
  Screenshots are in `docs/screenshots/android-jarvis/`.
- The installed Android 16.1 emulator image crashed in its own compositor
  (`hasReadColorBufferDma`); Android 16 runtime coverage remains unverified.
- Codex v0.160.0 installed successfully through the app's Coding agents page
  inside the Android guest. A real Jarvis start then returned the expected
  ChatGPT sign-in requirement and released its microphone foreground service.

When rebuilding an APK to update an existing debug installation, keep the
same `ANDROID_USER_HOME` and debug keystore as that installation. This host's
PR #700 build used the XDG Android directory (`~/.config/.android`), whereas
Gradle's unset default may select a different `~/.android` key. The delivered
trial APK uses the original #700 signing certificate so it can update that app.

## Default digital assistant and system overlay

In **Zeron Settings → Jarvis → Android assistant**, tap **Use Zeron as default
assistant**. In Android's **Digital assistant app** screen, tap **Default digital
assistant app**, select **Zeron**, and confirm. The setup page shows whether Zeron
holds the assistant role and lets you return to that system picker to change it.
Android's assistant role is not requestable with `createRequestRoleIntent` on
AOSP/Pixel, so this button opens `ACTION_VOICE_INPUT_SETTINGS` directly.

Configure your phone's assistant gesture. On Pixel, **Android Settings → System →
Gestures → Press and hold power button → Digital assistant** routes a power-button
hold to Zeron. Other manufacturers put this under gestures, side button, or
buttons; corner-swipe/hold-home assistant invocation also works where supported.
Zeron cannot change a manufacturer's hardware-button assignment itself.

From an unlocked home screen or another app, invoke that gesture. Android shows
Jarvis in a system assistant window above the current screen and starts the
configured voice call. A cold invocation boots the existing configured phone
runtime and waits for voice-host discovery. Repeated invocations attach to the
same live call. Finish Zeron's first-run setup and configure/sign in to the chosen
Codex host first; failures stay in the overlay with setup/retry controls.

The first call's audio/tool consent appears inside the overlay. A separate
non-exported translucent activity handles Android microphone permission. The
assistant window hides during that prompt (otherwise it would cover it), then
returns with the result. Permission requests carry the call generation across
rotation; cancellation, timeout, logout, and stale callbacks cannot restart a
call. The microphone foreground service starts only after microphone permission.

The overlay includes the shared orb, status, captions, mute, audio route, hang-up,
settings, and conversation controls. **Close**, outside tap, or Back dismisses the
overlay and ends its displayed call. **Keep call in background** explicitly keeps
the existing foreground call; its notification and another assistant invocation
let you return. Enable notification access in the overlay for background controls.
Opening the full app explicitly also keeps an existing call. Changing the default
assistant stops calls that originated through the assistant.

The top-level `VoiceInteractionService` only registers with Android. Selecting it
never boots an engine or opens the microphone; `ZeronApplication` initializes the
native model/fonts on first actual app/overlay use. Both system-bound voice
services require `BIND_VOICE_INTERACTION`. No accessibility service, overlay
permission, hotword service, or screen capture is added. Both the registration and
session disable `SHOW_WITH_ASSIST | SHOW_WITH_SCREENSHOT`; underlying app text,
URLs, and screenshots are not requested or sent. Locked invocations cannot start
a new call. On Android 10/11, which require an assistant recognition component,
ordinary system dictation is delegated to an installed speech provider instead of
being sent through Jarvis. Newer Android retains its original recognizer.

Primary platform references:
- [VoiceInteractionService](https://developer.android.com/reference/android/service/voice/VoiceInteractionService)
- [VoiceInteractionSession](https://developer.android.com/reference/android/service/voice/VoiceInteractionSession)
- [Assistant role behavior](https://android.googlesource.com/platform/packages/modules/Permission/+/refs/heads/main/PermissionController/src/com/android/permissioncontroller/role/ui/behavior/AssistantRoleUiBehavior.java)
- [Microphone foreground-service exemptions](https://developer.android.com/develop/background-work/services/fgs/restrictions-bg-start#wiu-restrictions)
- [Pixel gestures](https://support.google.com/pixelphone/answer/7443425)

### Assistant validation

Android 15 x86_64 emulator, with the actual system role qualification and picker
(no qualification bypass): selected Zeron, configured the power-button hold for
assistant, invoked with `input keyevent --longpress 26` over Android Settings and
home, and verified the underlying activity stayed in front beneath the overlay.
The live framework reports disabled screen context `3` (assist + screenshot).
Cold invocation, per-account consent, visible microphone prompt, denial/recovery,
grant, native Codex sign-in-required result, foreground-service cleanup, and Back
were exercised. Pressing Home during the microphone prompt cancelled the pending
call and removed its permission task without reopening the overlay or starting
the microphone service. All 272 JVM tests and four Android instrumentation tests
(two real JNI audio tests and two assistant policy/manifest tests) passed. Lint
retains the same 12 pre-existing errors outside voice; no assistant lint errors. Screenshots are in `docs/screenshots/android-jarvis-assistant/`,
including light mode with 200% font scale. Real JNI audio tests remain independent
of provider credentials. The legacy Android 10/11 dictation delegate, individual
manufacturer gestures, physical-device acoustics/Bluetooth, and a signed-in
OpenAI conversation remain unverified on hardware.

### Minimal assistant overlay and camera context

Power hold now shows transparent, bottom-aligned content over Android's
full-screen dim layer (`FLAG_DIM_BEHIND`, dim amount 0.76). There is no rounded
card, background fill, or underlying-app blur. The current app stays visible;
Zeron does not capture or read it. The overlay's orb uses Zeron purple
`#8B7CF6`; the ordinary in-app orb retains its existing appearance. Overlay
text and controls use a dark palette in either phone theme so they remain
readable over the dimmed screen.

The Jarvis title, close icon, host label, persistent listening/status text, and
instructional footer are removed. Settings sits at bottom left, camera at
bottom right, and the background-call action is an icon between them. Mute,
audio route, and hang-up controls remain. Captions, consent, transient progress,
errors, and recovery actions remain when relevant. Launch routes are consumed
only by a resumed activity: Android's assistant task must not lose its settings
or conversation request to a hidden Zeron activity.

During an active call, tap the camera, capture a picture in the phone's camera
app, and confirm it. **Photo added** means the execution host adopted the image
message. Ask about it aloud; Codex's visual analysis becomes context for Jarvis.
If Codex is already working, the image stays on the normal message queue and the
overlay says **Photo queued**, preserving the existing work. Analysis may take a
few seconds. The photo and its inspection request are normal, persistent image
attachments in the same Zeron voice chat.

The system camera owns capture through `ACTION_IMAGE_CAPTURE` and `EXTRA_OUTPUT`.
Zeron requests no camera or storage permission. A non-exported bridge grants
read/write access to one private FileProvider cache URI, then revokes that grant
and deletes the capture file. EXIF-aware decoding limits the longest edge to
1280 pixels; adaptive JPEG compression limits the attachment to 768,000 bytes.
Re-encoding removes camera/location metadata. The existing Zeron attachment
transport delivers the image to the selected execution host. Its Codex thread
analyzes the image and automatically forwards that result into V3's voice
context. No raw image or public `conversation.item.create` event is sent over
the audio data channel. This uses the existing ChatGPT subscription/Codex
connection, with no separate API key or image service.

The inspection prompt asks for factual visible details and relevant readable
text, permits only the image viewer, forbids other tools/actions, and treats
image content as untrusted context.
Local submission is not mistaken for host adoption. Failed transfers report a
safe error; a 30-second unconfirmed submission reports **Photo pending** and asks
the user to check the conversation before retrying. There is no automatic retry.

Microphone capture pauses while the camera/photo submission is active and
resumes according to the current user mute setting. A call-generation and random
capture token are checked before encoding and again before submission, preventing
late captures from reaching a replacement call, including after process restart.
Camera-bridge rotation retains a single delivery job. Home, lock, call termination,
or navigation away revokes ownership. Once a photo has been submitted, it remains
an ordinary chat attachment even if the call ends; capture-file cleanup does not
delete that submitted attachment. The camera button is unavailable before the
call becomes active and while a photo is being submitted.

References: [Codex V3 context append implementation](https://github.com/openai/codex/blob/afb436df8b70bb5bc57b86d9a3e829968988cd21/codex-rs/codex-api/src/endpoint/realtime_websocket/methods_frameless_bidi.rs)
and [Android camera intents](https://developer.android.com/guide/components/intents-common#Camera).

Validation for the current overlay/photo implementation:

- All 280 JVM tests pass, including attachment intent and queue policy,
  correlated host adoption, upload failure, stale ownership, cancellation,
  timeout without retry, and camera mute precedence.
- Seven Android instrumentation tests cover native camera capture into the
  production private-URI contract, image decoding/encoding and provider
  isolation, real JNI audio cleanup, assistant protection/ownership, and
  accessible control placement. The emulator camera test starts a fresh capture
  task and waits for input-service commands to finish before moving on.
- Actual power-button invocation over light and dark Android Settings shows a
  transparent `VOICE_INTERACTION` window with `DIM_BEHIND` and no `BLUR_BEHIND`.
  The framework still reports disabled screen context `3`. At 200% system text
  scale, recovery text and bottom controls remain visible and reachable. The
  settings action opens Jarvis settings on repeated invocations with an existing
  stopped Zeron task.
- Screenshots in `docs/screenshots/android-jarvis-assistant-minimal/` distinguish
  actual system invocations from an active-call presentation fixture. Native
  capture uses the emulator's virtual camera.
- Lint retains its 12 existing errors and 248 warnings. The update APK retains
  the existing signing certificate (SHA-256
  `ca8b677131bd3beed4986da25ce3110fe34144fb1bef136d5633fd65a697e903`).

Physical-phone camera variants and the full signed-in Android capture/upload
lifecycle remain unverified. Signed-in Codex/V3 vision behavior was verified
separately on the actual provider connection, as described below.

### V3 provider rejection correction

The earlier data-channel implementation used the public Realtime API's
`conversation.item.create`/`input_image` format. A real signed-in Codex 0.159.2
V3 call rejected it with `invalid_value`, parameter `type`: **Invalid value:
'conversation.item.create'**. Codex forwarded this as `thread/realtime/error`
and closed the call. A `session.context.append` image part was also rejected
because V3 requires text there. The confirmed Android JSON slash-escaping size
bug fixed in `542da350` was real, but fixing it did not solve this protocol
mismatch. The direct photo data-channel sender and its simulated-ACK tests have
now been removed.

Live validation used an ephemeral local Codex thread, a fresh headless WebRTC
peer, synthetic silent audio, and a generated image; no physical microphone,
private user photo, new API key, or messages to other chats were involved.
The supported route submitted the image as a normal `localImage` input to the
same Codex thread during its active V3 call. Codex's completed analysis was
**The image shows a solid purple triangle pointing upward on a white background.**
Its V3 sideband automatically appended that analysis to the voice context. When
asked about the image, Jarvis's final voice transcript correctly described the
purple triangle and white background, without a provider error or call closure.

Android now uses the existing Zeron attachment escort and normal queued send to
reach that same Codex thread. Zeron's current Codex harness presents attachments
as local file paths in the prompt; the inspection prompt explicitly permits
opening that file with the image viewer. A second live check used precisely this
text-only attachment trailer and the production inspection prompt, without a
`localImage` input. Codex inspected the file and reported a solid purple triangle
on white with no visible text. V3 automatically received that result and spoke
the correct description while the call remained active.

A queued image never interrupts existing
work. A directly submitted image waits for its exact host-adopted user message
rather than claiming success when the local send method returns. These
submission/ownership semantics are covered by the added JVM tests; the signed-in
provider image-to-voice behavior is the separate live result above.
