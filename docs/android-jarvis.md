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
