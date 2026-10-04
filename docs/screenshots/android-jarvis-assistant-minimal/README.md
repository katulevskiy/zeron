These are Android 15 emulator captures, not the user-provided phone images.

- `power-hold-light.png` and `power-hold-dark.png`: actual system assistant
  invocation over Android Settings. The emulator has no ChatGPT sign-in, so
  required recovery text is visible and the camera is correctly disabled.
- `power-hold-200-percent-text.png`: the same actual invocation at 200% system
  text scale, with reachable settings/camera controls.
- `active-call-presentation-fixture.png`: the production stateless overlay
  composable with an active-call fixture on a dark test backdrop. Demonstrates
  captions, purple orb, mute/route/hang-up and settings/minimize/camera placement;
  it is not a signed-in call or evidence of provider image recognition.

Native capture into Zeron's private photo URI and the real WebRTC photo/ack path
are covered separately by the Android instrumentation suite. See
[`android-jarvis.md`](../../android-jarvis.md) for validation limits.
