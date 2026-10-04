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

### Frosted assistant surface

The assistant follows the **phone's system light/dark theme**, independently of
Zeron's app appearance preference. A rounded light or dark glass surface lets
the current app show through. Captions, status and icons remain sharp, with
opaque call-control buttons and stronger action/error colors for contrast.

On Android 12+ the assistant requests the supported system **blur behind** API
(`FLAG_BLUR_BEHIND`, `setBlurBehindRadius`, 24 dp converted to pixels). Android's
compositor blurs the app beneath the full assistant window; Compose does not
blur assistant content, take a screenshot, or read screen context. The glass
tint is 78% opaque with blur. On older devices, unsupported GPUs, or when Android
disables blur (including battery saving), it becomes 90% opaque with a slightly
stronger scrim. Both paths retain translucency. A cross-window blur listener
updates this immediately and is removed when the window detaches or the session
is destroyed. See the [Android blur API](https://developer.android.com/reference/android/view/WindowManager.LayoutParams#setBlurBehindRadius(int))
and [runtime blur listener](https://developer.android.com/reference/android/view/WindowManager#addCrossWindowBlurEnabledListener(java.util.function.Consumer%3Cjava.lang.Boolean%3E)).

Validation for this surface update:

- Android 15 emulator reports blur supported and enabled. Actual power-button
  invocation over Settings showed blurred underlying content and sharp Jarvis
  UI. The live assistant window reported `BLUR_BEHIND`, radius 63 px at 420 dpi.
- Switching the system blur setting off while the overlay remained open removed
  the flag/radius and immediately strengthened its translucent tint. Re-enabling
  blur restored the glass effect. Light/dark system theme changes also updated
  the open overlay; 200% text remained readable with reachable controls.
- The added platform instrumentation test exercises real enable/disable
  callbacks, window attributes, and listener cleanup. All five instrumentation
  tests passed, including the existing JNI audio and assistant policy tests.
  All 272 JVM tests passed. Lint retains the same 12 existing errors outside
  voice; this update introduces no lint findings.
- Worst-case composited white/black underlays give normal text at least 8.32:1,
  text actions/errors at least 4.84:1, and filled control labels at least 8:1.
- Screenshots in `docs/screenshots/android-jarvis-assistant/` include light/dark
  frosted surfaces and both disabled-blur fallback themes.
- The update APK retains the existing trial's signing certificate (SHA-256
  `ca8b677131bd3beed4986da25ce3110fe34144fb1bef136d5633fd65a697e903`).

Physical-phone compositor appearance, unsupported-GPU hardware, and Android
10/11 remain untested for this update. The emulator was not signed in to ChatGPT;
it exercised the native sign-in-required result and cleanup, while the local
JNI tests covered audio without provider credentials.
