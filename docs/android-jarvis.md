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
services require `BIND_VOICE_INTERACTION`. There is no draw-over-other-apps
permission or hotword service. Screen capture and accessibility control are
separate, optional experimental capabilities described below. By default both
screen-context flags are disabled; opting into screen context allows assistant
screenshots. `SHOW_WITH_ASSIST` remains disabled: Jev's UI structure comes from
the separately enabled accessibility service. Locked invocations cannot start
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
Zeron captures or reads it only through the separately enabled screen features
described below. The overlay's orb uses Zeron purple
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

### Circular camera and uninterrupted photo delivery

During an active call, tap the camera icon. The purple orb grows into a circular
live CameraX preview inside the same dimmed assistant window; six iris blades
open after the first actual frame. Tap the 64dp shutter once. There is no camera
app, confirmation screen, activity handoff, microphone pause, or upload wait.
The camera returns to the orb as soon as its JPEG is saved, while the session
encodes and delivers the photo asynchronously. Mute, speaker, and hang-up remain
available throughout. Back closes the camera first; another Back ends the overlay.
Animations honor Android's reduced-motion setting, and the completed iris is
removed so it cannot obscure the viewfinder if a rendering frame stalls.

Zeron now asks for CAMERA permission once, on the explicit camera action, through
its non-exported permission activity. It still requests no storage or draw-over-
other-apps permission. Screen context requires its separate opt-in. CameraX is bound to the assistant's
visible lifecycle only. Home, lock, minimize, hang-up, navigation, or session
destruction unbinds its own camera use cases. Stale provider/capture callbacks
cannot re-open the preview or deliver into a successor call. The camera provider
can initialize during an active voice call, but no camera opens until the user
requests it. Devices without a rear camera fall back to their front camera.

CameraX uses zero-shutter-lag capture where supported, its minimize-latency
fallback otherwise, flash off, and a preferred 1280x960 still size. The encoder
honors image orientation, strips camera/location metadata, limits the longest
edge to 1280 pixels, and bounds JPEG bytes to 384,000 (one upload chunk). Files
stay in private cache until delivery finishes, then are deleted. Cancelled late
captures are deleted; old orphan captures are cleaned when camera setup begins.
Three independent photo deliveries may be pending; duplicate shutter taps are
ignored during capture. Preview loading, capture, encoding, upload, and adoption
never alter microphone mute or audio focus.

The stuck **Photo added** call had two causes in the previous design. It handed
control to a separate camera activity and locally paused capture. More critically,
`RuntimeConfig::can_route` rejects every nonempty `RunRequest.attachments`,
replacing the parked Codex process and its V3 bridge when the photo is sent.
Android now uploads through the existing `CoreClient.uploadAttachment` to the
captured call's execution host first. Only after that host confirms an absolute
local path does it send the usual image-path trailer, with **no duplicate
RunRequest attachments**, to the existing conversation. The current host can
route that text into the same warm process, preserving the live voice lease;
no desktop-host upgrade is required.

**Photo added** means the host adopted the context message, not that vision
analysis has finished. **Photo queued** preserves an existing turn. The image
viewer reads the confirmed host file, and Codex's analysis automatically becomes
V3 voice context. The prompt allows the image viewer only and treats image
contents as untrusted. Upload failure, stale ownership, malformed/pending paths,
and unconfirmed adoption do not claim success or trigger an automatic retry.
Photo notices clear after two seconds; a stale success cannot mask a stopped
call's Start/recovery controls. Once submitted, the uploaded photo and context
remain part of the conversation; cancelling local capture does not erase them.

Connection setup also overlaps local WebRTC SDP/ICE gathering with execution-host
startup/probing. Permission and native preparation still precede both, ownership
and heartbeats attach before waiting for a slow offer, and microphone activation
still waits for valid negotiation and confirmation. Cancelling or failing either
branch closes media and cleans the remote attempt. This removes their former
serial delay; network/provider startup still takes time. Debug logs expose only
stage names and elapsed milliseconds (`JarvisConnection`, `JarvisCamera`).

Validation:

- 281 JVM tests pass, including upload-before-submit ordering, call replacement,
  exact host adoption, failed/unconfirmed delivery, queue policy, cancellation,
  malformed paths, and user mute precedence.
- Eight Rust voice-session tests pass, including a two-party rendezvous that
  proves setup overlaps while microphone activation waits for confirmation,
  cancellation, and heartbeat expiry during a stalled offer.
- The engine's remote voice regression sends a landed photo path into the
  persistent Codex runtime, verifies the same run/thread, no realtime stop, and
  a valid media-report acknowledgement afterward. It uses an offline Codex
  fixture; provider recognition is the separate live check below.
- Seven Android instrumentation tests cover CameraX capture while actual JNI
  WebRTC microphone recording remains active during preview/capture/encoding,
  real assistant-window camera streaming over Settings, Back closing camera
  without dismissing the session, native audio cleanup, image encoding/provider
  isolation, assistant protections, and accessible controls.
- The real assistant camera also streams at 200% system font size; screenshot
  review confirms the circle, shutter, settings and camera-close actions remain
  reachable. This is a call-free camera fixture over Settings, not a signed-in
  call at that text scale.
- The emulator measured roughly 260–340ms shutter-to-saved callback and about
  310–520ms including test-side JPEG encoding. These are virtual-camera timings,
  not physical-phone or end-to-end connection measurements.
- Both Android native core ABIs are rebuilt; lint retains 12 existing errors and
  248 warnings. Signing certificate SHA-256 remains
  `ca8b677131bd3beed4986da25ce3110fe34144fb1bef136d5633fd65a697e903`.
- `docs/screenshots/android-jarvis-circle/` distinguishes the live-audio camera
  fixture from actual assistant camera invocation over Settings. Neither is a
  full signed-in Android provider call. The earlier minimal-overlay screenshots
  remain historical evidence, including the 200% text-scale check.

Physical-phone camera behavior, full signed-in Android capture/upload/audio
round-trip, and actual connection latency remain unverified on the phone.

References: [CameraX preview](https://developer.android.com/media/camera/camerax/preview),
[zero-shutter-lag and fallback](https://developer.android.com/media/camera/camerax/take-photo/zsl),
and [Codex V3 context append](https://github.com/openai/codex/blob/afb436df8b70bb5bc57b86d9a3e829968988cd21/codex-rs/codex-api/src/endpoint/realtime_websocket/methods_frameless_bidi.rs).

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

The Android image-path context targets that same Codex thread after the host
file upload completes, as described above. Zeron's current Codex harness presents attachments
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


## Experimental screen context and Jev phone control

This fork adds three independent experiments without pausing the voice call:

1. In **Jarvis settings → Phone screen**, enable **Screen context**. Invoke
   Jarvis over another app, tap the screen icon, then **Share screenshot**.
   A fresh assistant screenshot can be used without accessibility access;
   subsequent on-demand captures use Android's accessibility screenshot API.
   Android 14+ captures the underlying application window, excluding Jarvis.
   Android 11–13 hides the assistant before display capture. Android 10 can
   use an invocation screenshot or MediaProjection.
2. Tap **Start live screen sharing**, accept Android's capture dialog, and
   choose **Entire screen** to follow app changes (or a single app to restrict
   capture). Local capture keeps just one frame, sampled at up to four frames
   per second. Changed screenshots are forwarded through the existing landed
   image/Codex vision path at most once every two seconds, with one transfer
   in flight. Busy/queued work delays delivery and conflates to the latest
   frame instead of building a screenshot backlog. This is periodic vision
   context, not a Realtime video stream. The assistant's own animation is not
   forwarded; live delivery pauses while that overlay is open. Stop sharing
   from the screen controls or its ongoing Android notification.
3. For interaction, choose **This phone** as Jarvis's execution device, enable
   **Phone control**, enter your own TypeSafe Jev key, and use **Test key**.
   Enable **Jarvis phone control** under Android Accessibility. During a call,
   tap the screen icon and **Enable control for this call**. Open the app you
   want to work in and give Jarvis a task, such as “open Display in Settings,”
   “put this exact text in the search field,” or “tap x=400 y=800.” The overlay
   hides before input so it cannot intercept taps; voice remains active.
   **Try with Jev** in the settings screen returns to the preceding app before
   running the supplied goal. Stop control in Jarvis or the call notification.
   If Android restricts accessibility for the sideloaded APK, allow restricted
   settings in Zeron's App info menu before enabling the service. Jarvis settings
   include **Open Zeron App info**: choose **⋮ → Allow restricted settings**,
   approve Android's prompt, then return to **Accessibility → Jarvis phone control**.
   The app cannot grant this Android approval itself. See
   [Google's restricted-settings instructions](https://support.google.com/android/answer/12623953).

The overlay's screen controls render inside its existing system-owned assistant
window. A separate Compose Dialog is not valid in that window context: Android
rejects an app window (type 2) inside the voice interaction context (type 2031).
Back or Done closes the screen controls while preserving the call. Opening the
controls closes an open camera preview; hiding the assistant resets the panel.
Live sharing uses its own capture consent, independently of accessibility.

The Jev key is entered on the phone, masked in a secure dialog, and encrypted
using an Android Keystore AES-GCM key. It is not passed to Codex, the guest,
chat prompts, model contexts, logs, source files, or build configuration. App
backup remains disabled. API calls go only to TypeSafe's official
`https://api.typesafe.ai/v1/systemone`; no third-party Jev proxy is used.

GPT/Codex plans tasks and supplies exact text. The phone reads bounded visible
accessibility elements, labels, supported actions and physical screen bounds.
Jev is text-only: it selects the operation and speculative per-operation
candidate target in one request. Only the selected target head is validated
and executed. Screenshots go to GPT/Codex for visual understanding; unsupported
canvas controls can use visually grounded coordinate taps, not invented targets.

The design follows the primary-source patterns in
[Browser Use's Jev Ultrafast](https://github.com/browser-use/jev-ultrafast),
[jev-bot](https://github.com/stoopid-computers/jev-bot), and
[TypeSafe's speculative fan-out](https://docs.typesafe.ai/patterns/fan-out).
It does not copy their reported latency claims to Android.

A per-call loopback bridge lets the existing on-phone Codex exec tool request
state, screenshot, bounded Jev subtasks, coordinate taps/swipes and revocation.
It binds only `127.0.0.1`, uses a random 256-bit capability, rejects browser
Origins, and serializes input. No LAN port or persistent control token is
exposed. Its instructions are appended to the existing voice chat as plain
text with no RunRequest attachments, preserving the warm Codex voice runtime.
Screenshot files are private, bounded to the latest five, and removed on
revocation. The Jev key never leaves the Android key store except the official
HTTPS Authorization header. Remote desktop-hosted Jarvis can receive screen
images, but cannot use this local interaction bridge in this initial experiment.

Control checks the call generation, settings and unlocked state. Decisions
must contain exactly the offered choices with finite probabilities/confidence;
uncertain or malformed output produces no input. Before execution, the screen
fingerprint/age, target identity, current label/value/role and bounds are
rechecked. Android's node cache is cleared once per snapshot on Android 13+;
older versions refresh each node. Stale selections can replan within the
bounded decision budget without executing input. There are at most eight
requests per subtask and a stall check; a model's DONE is explicitly
`done_unverified` until GPT independently inspects the outcome. Screen text is
untrusted context. Password fields and Zeron's own application are excluded
from control observations. Secure windows remain protected from capture.

Screen sharing and control stop on hang-up, lock, identity change, or process
loss. Disabling accessibility stops interaction; screen sharing has its own
separate stop control. Changing preferences does not silently start capture or
input, and neither feature restarts automatically after process death.

Platform references:

- [AccessibilityService](https://developer.android.com/reference/android/accessibilityservice/AccessibilityService)
- [MediaProjection consent and lifecycle](https://developer.android.com/media/grow/media-projection)
- [TypeSafe request/answer contract](https://docs.typesafe.ai/api)
- [Jev text-only model limits](https://docs.typesafe.ai/models)

Validation for the screen experiment (API 35 emulator):

- 285 JVM tests pass, including malformed/uncertain decision rejection,
  speculative target selection and stale/rotated/out-of-bounds input checks.
- The final voice instrumentation suite passes (14 reported tests, with the two
  optional live-provider/planner checks skipped and verified separately).
  Real Android tests cover
  clicks, typing, coordinate gestures, scrolling/Home/Back, password redaction,
  secure screenshot rejection, system-panel occlusion, encrypted BYOK and bridge
  authentication/revocation. Optional live-provider checks need a private runtime
  credential; no provider credential is part of either APK.
- Actual MediaProjection consent/capture receives changing frames while the
  negotiated JNI microphone remains active, and stopping capture releases its
  resources without interrupting the microphone.
- A live Jev request selected and executed the intended native Android button
  in 560 ms, with its changed UI independently verified.
- A real GPT/Codex CLI planner used the production native bridge handler and
  Jev to enter `pizza` and select `Mark complete` in the test fixture. Both
  outcomes were verified; Jev's two decisions took 208 ms and 111 ms in the final
  run. The test uses a fixture call state, not a signed-in voice session.
- Signed APK includes ARM64 and x86_64 and matches the previous update signing
  certificate. Decompressed APK contents have no embedded Jev credential.
- Lint still reports 12 pre-existing errors outside this feature. Physical-phone
  testing, signed-in V3 voice-to-control execution and model delivery of live
  screenshots remain to be verified on the user's device. These are changed
  screenshots delivered periodically, not a native real-time video model feed.

The screen-button crash was reproduced by clicking the real power-hold overlay
button on API 35. The regression exercises that button, the owned window type,
Back preserving a fixture call, missing accessibility producing a message, and
the live-sharing capture-consent handoff. The fixture starts no provider call.
