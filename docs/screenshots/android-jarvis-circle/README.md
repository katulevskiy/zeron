These are Android 15 emulator images, not user-provided phone photos.

- `camera-with-live-audio-fixture.png`: production circular camera composable,
  actual CameraX preview during iris opening, and simultaneous native WebRTC
  microphone recording.
  Call captions/state use a fixture; no signed-in provider session.
- `system-camera-over-settings.png`: actual system assistant window over Android
  Settings. The test opens the production camera controller directly because
  the emulator has no signed-in voice host; it does not validate an authenticated
  call or the normal permission-then-camera action.
- `system-camera-200-percent.png`: the same actual window and circular preview
  during iris opening at 200% system text size. The camera lifecycle test passes
  and controls remain reachable; the emulator has no authenticated voice call.

See [validation details](../../android-jarvis.md). Physical-phone camera quality,
latency, and full signed-in Android photo-to-voice behavior remain unverified.
