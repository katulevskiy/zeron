# Android: sounds and haptics

The phone's sensory layer: how Zeron for Android feels and sounds. It reuses the
desktop's notification family (`done`, `request`, `attention`, see
[README.md](README.md)) for session events, promotes five of the desktop
auditions, and adds fifteen small interface cues in the same "rounded pressure
pulse" language. Code: `apps/android/app/src/main/java/sh/zeron/android/feedback/`.
Asset QA: [android-audit.md](android-audit.md).

## Principles

- **Restraint.** Feedback answers a person or announces a one-shot event. There
  is none on scrolling, none per recomposition, none on typing, none for a state
  that merely redraws. Interface cues sit 2 to 4 dB under the session chimes
  (RMS -44 dBFS over their active part against -40 to -42) and last 34 to 176 ms
  against 520 to 650, so they read as far quieter; the shortest are under 40 ms.
- **Consistency.** One vocabulary (`Feedback.kt`: `Haptic`, `Cue`) names the
  *moment*, never the vibration. A tap is always `Select` + `Tap`, a choice
  `Select` + `Select`, an irreversible step `Heavy` + `Delete`. Screens never
  build waveforms.
- **Meaning by pitch.** Rising means opening, turning on, adding, refreshing.
  Falling means closing, turning off, removing, failing. Lower and heavier means
  more destructive (Delete is the lowest cue). The audit checks the directions
  numerically (Open rises, Close falls, ToggleOn rises, ToggleOff falls).
- **Haptic and sound as a pair.** Every cue has a partner haptic that lands on
  the same instant and is just as large (a soft tap, a click, a rising pair, a
  thud). Either channel is complete without the other: silent mode keeps
  haptics, haptics off keeps sounds.
- **One key.** Everything is built from the C major pentatonic scale (C D E G A),
  which cannot clash with itself or with the desktop chimes. The slider detent is
  one pure 880 Hz tick resampled up a pentatonic ladder, so dragging "plays"
  the scale.
- **Accessibility.** Both channels have independent switches; haptic strength
  scales; the system's touch-vibration and touch-sound settings are honoured;
  nothing is the only carrier of information (every cue accompanies visible
  state).
- **Battery and calm.** Sounds are preloaded in a `SoundPool` (no decoding on
  press, no audio focus, nothing held open). Haptics are short; light ones are
  rate-limited so a fast drag never buzzes continuously.

## When it plays

| Rule | Detail |
| --- | --- |
| App active | Only while the app is in the foreground **and** the screen is on. Otherwise nothing plays in-app. Events that matter in the background arrive as notifications (below). |
| Haptics | Needs the in-app **Haptics** switch, a vibrator, and the system's *Touch feedback* setting. The ringer mode does **not** matter (Android's own keyboard and touch haptics also keep working on silent). |
| Sounds | Needs the **Sounds** master, the category switch, a normal ringer (silent and vibrate mute everything), no total-silence / alarms-only Do Not Disturb, and a non-zero system sound volume. Interface cues also follow the system's *Touch sounds* setting; session chimes do not (they are events, not touch feedback). Priority-only Do Not Disturb does not block them (they are sonification, like keyboard clicks). |
| No audio focus | Cues are `USAGE_ASSISTANCE_SONIFICATION` / `CONTENT_TYPE_SONIFICATION` and never take focus, so music keeps playing and nothing ducks. |
| Rate limits | The same cue never stacks inside its own gap (60 to 400 ms by cue); any two cues of equal or lower priority keep 20 ms apart; at most 4 concurrent streams (a more important cue still gets in); the same haptic is coalesced (70 ms for ticks up to 400 ms for alerts); at most 12 light haptics per second; any alert cuts through light ticks. |
| Default tap | Plain controls answer `Select` + `Tap` *after* their action; if the action asked for feedback of its own (a toggle, a menu item, a navigation's Open cue) the default stays out of the way. Menus and popovers close silently after a chosen item. |

## Haptic mapping

`HapticDesign.kt` walks a ladder per haptic: the platform's own
`performHapticFeedback` constant (OEM tuned, so it feels native) when the
strength is Standard, then a `VibrationEffect.Composition` of primitives (only
those `areAllPrimitivesSupported`), then the designed waveform (amplitude control),
then a predefined effect, then plain on/off timings. minSdk 29, so every API-gated
constant has a fallback.

| Haptic | Moment | Standard (API 34+) | Primitives | Fallback |
| --- | --- | --- | --- | --- |
| `Tick` | slider / detent, terminal key, disclosure | `SEGMENT_FREQUENT_TICK` (`CLOCK_TICK` below 34) | LOW_TICK 0.5 | `EFFECT_TICK` / 10 ms @40 |
| `Select` | tap, tab, choice | `SEGMENT_TICK` (`CONTEXT_CLICK`) | TICK 0.7 | `EFFECT_TICK` |
| `ToggleOn` | switch on | `TOGGLE_ON` | LOW_TICK 0.5 then TICK 0.8 (rising pair) | `EFFECT_CLICK`, two-step waveform |
| `ToggleOff` | switch off | `TOGGLE_OFF` | TICK 0.7 then LOW_TICK 0.4 (falling pair) | `EFFECT_TICK`, two-step waveform |
| `Press` | press-and-hold begins | `GESTURE_START` | CLICK 0.4 | `EFFECT_TICK` |
| `Confirm` | send, save, create, start | `CONFIRM` | CLICK 0.65 | `EFFECT_CLICK` |
| `Success` | turn finished, transfer arrived | designed | QUICK_RISE 0.45, LOW_TICK 0.55 @40 ms | CLICK+TICK / waveform |
| `Attention` | a question, connection lost | designed | TICK 0.85, TICK 0.85 @90 ms | `EFFECT_DOUBLE_CLICK` |
| `Error` | failure, refusal | `REJECT` only as fallback | CLICK 1.0, THUD 0.8 @60 ms | `EFFECT_HEAVY_CLICK` |
| `LongPress` | long press recognised | `LONG_PRESS` | CLICK 0.7 | `EFFECT_CLICK` |
| `Threshold` | swipe or pull crossed its commit point | `GESTURE_THRESHOLD_ACTIVATE` | CLICK 0.55 | `EFFECT_CLICK` |
| `Heavy` | delete, uninstall, discard | designed | THUD 0.85, CLICK 0.5 @30 ms | `EFFECT_HEAVY_CLICK` |
| `Pop` | pinned, starred | designed | TICK 0.8, LOW_TICK 0.35 @25 ms | `EFFECT_TICK` |

**Strength** (Settings): *Subtle* scales primitives and waveform amplitudes by 0.5,
*Standard* by 1.0 (and prefers the platform constants), *Strong* by 1.5 (clamped
to full). Platform constants have a fixed strength, so Subtle and Strong use the
composition path; hardware with no amplitude control and no primitives cannot
honour Subtle, so it drops only the lightest (scroll-class) haptics rather than
playing them at full strength. On the emulator's vibrator the Strong / Subtle
Confirm click is played at scale 0.97 / 0.32 (`dumpsys vibrator_manager`).

## Sound cues

Files are `res/raw/fx_<name>.wav`. Interface cues are generated
(`scripts/generate-android-sounds.py`, deterministic, mono 16-bit 48 kHz) and
committed; `done`, `request`, `attention` are copied byte-for-byte from
`crates/ui/assets/sounds` by the Gradle `genSounds` task; the five promoted cues
come from `docs/sound-design/auditions/` (stereo to mono, level untouched).
Category = the in-app switch that governs it. "Trigger" lists every place the
cue is wired.

| Cue | Source | ms | Category | Paired haptic | Trigger(s) |
| --- | --- | ---: | --- | --- | --- |
| `Done` | desktop `done.wav` | 520 | Completion | `Success` | a session's turn finished while Zeron is open |
| `Request` | desktop `request.wav` | 600 | Input required | `Attention` | a session is waiting for an answer or approval |
| `Attention` | desktop `attention.wav` | 650 | Errors | `Error` / `Attention` | a session failed; connection lost while a turn runs |
| `Send` | desktop audition 01 | 300 | Interface | `Confirm` | composer send / steer, question answer, queued message "Send now" |
| `Queued` | desktop audition 02 | 480 | Interface | `Confirm` | composer send while a turn is running (queue) |
| `UploadReady` | desktop audition 03 | 740 | Interface | `Success` | Save to Downloads done; agent install / update done; project cloned or created |
| `Reconnected` | desktop audition 09 | 760 | Errors | `Confirm` | connection restored after a loss that was announced |
| `Undo` | desktop audition 10 | 490 | Interface | `Select` | Undo on the "Archived" snackbar |
| `Tap` | generated | 34 | Interface | `Select` | default tap of any plain control; links, tool disclosures, file badges, tree rows |
| `Select` | generated | 58 | Interface | `Select` | tabs, filter pills, theme / strength choice, menu choices, mentions picked, photos attached, file saved, jump to latest, rename confirm, paste in terminal |
| `ToggleOn` / `ToggleOff` | generated | 72 / 66 | Interface | `ToggleOn` / `ToggleOff` | every switch, question option chips, Settings toggles |
| `Open` | generated | 108 | Interface | none (the tap's) | a page pushed, a sheet / dialog / menu / popover opens, a section expands, a terminal tab opens, the demo or the developer door opens |
| `Close` | generated | 100 | Interface | none | back, a sheet / dialog / menu closes (quiet after a chosen item), a section collapses, stop a run, close a terminal tab, remove a staged photo, leave the demo / sign out |
| `Detent` | generated, pitch-shifted | 38 | Interface | `Tick` (effort: `EffortStep`, firmer per level) | effort levels (climbs the ladder per level), the volume slider (ten steps) |
| `Star` / `Unstar` | generated | 108 / 92 | Interface | `Pop` | favorites (the model picker); `Unstar` also unpins a session |
| `Pin` | generated | 84 | Interface | `Pop` | pin a session (list menu, session menu) |
| `Archive` | generated | 112 | Interface | `Confirm` | archive by swipe, menu, session menu or search |
| `Delete` | generated | 124 | Interface | `Heavy` (`Confirm` for light removals) | uninstall confirm, discard edits, sign out of an agent, remove a queued message, remove the wallpaper |
| `Copy` | generated | 76 | Interface | `Confirm` | every copy action: paths, links, transcript, code blocks, terminal selection |
| `Error` | generated | 176 | Interface | `Error` | a refusal or failure: save failed or conflicted, send failed, install / update / save-to-Downloads failed, terminal could not open, toasts |
| `Refresh` | generated | 118 | Interface | `Select` / `Confirm` | pull to refresh, reload, retry, refresh files |

Detent gets `Haptic.Tick` (on the effort slider `Haptic.EffortStep` with `level = index / (n - 1)`);
pull and swipe thresholds have haptic only (`Threshold`).

Model picker (compact card): the effort bar adds `Stretch` (once on entering the
rubber band, once at the wall), `Rebound` + `Cue.Rebound` on release, and, once the
user has settled on an end (180 ms dwell, or right after release), `Surge` +
`Cue.Surge` at the top and `Zip` + `Cue.Zip` at the bottom. The fast button plays
`Lightning` + `FastOn` when it turns on and `FastOff` + `Tick` when it turns off.
The provider rail answers a tap with `Select` + `Cue.forProvider(harness)`, moving
the finger onto the next provider (or scrolling the list across a section) with
`RailTick` (plus the provider's cue when scrubbing). Arrival and lightning effects
are skipped, not the feedback, when animations are off.
Not every vocabulary entry is wired by this layer: `Star`, `Pop` for favorites and
the model chip are used by the model picker, `Press` is reserved.

## Where each event goes

| Event | In the foreground | In the background |
| --- | --- | --- |
| Turn finished | `Success` + `Done` (Completion switch) | notification, channel sound `fx_done`, vibration 24-40-30 |
| Question / approval | `Attention` + `Request` (Input switch) | notification, `fx_request`, 18-90-18 |
| Session failed | `Error` + `Attention` (Errors switch) | notification, `fx_attention`, 35-55-45 |
| Connection lost mid-turn | `Attention` + `Attention` | nothing (nobody can hear it) |
| Connection restored | `Confirm` + `Reconnected`, only if the loss was announced | nothing |

Session events are derived from workspace snapshots (`SessionTransitions`): the
first snapshot is a baseline, subagent rows are skipped, new rows are baselines,
and the same event for the same session within 2 s is dropped. Stopping a run
yourself is not a completion. Returning to the app re-baselines silently: what
happened while away already had its notification. Periodic (clock) refreshes never
announce, so a status that merely aged out is not an event.

**Notification channels.** Channel sounds are immutable once a channel exists, so
session channels are versioned (`session-<kind>-v1-<s|v|sv|q>`); bumping
`Notifier.CHANNEL_VERSION` migrates (deletes) older ones. One channel exists per
kind and per sound / vibration combination the in-app switches ask for, created
lazily, so turning a chime off in Zeron also turns it off in the notification.
Sounds are `android.resource://sh.zeron.android/raw/fx_*`. Notifications need the
`POST_NOTIFICATIONS` permission (API 33+), asked once after the first message you
send and again from Settings. Android freezes or kills a backgrounded process, so
a session finishing long after you leave is only announced if the connection is
still alive; a push service would be the way to make that reliable (not part of
this change).

## Settings

Settings, then **Sounds & haptics**: master **Sounds**, **Interface sounds**, a
ten-step **Volume** (perceptual curve: gain is volume squared), the **Session
sounds** master with independent **Completion**, **Input required** and **Errors**
(mirroring the desktop's Settings, Notifications), **Background alerts**
(notification permission), **Haptics** with **Strength** (Subtle / Standard /
Strong) and a **Try them** list that plays the real cue and haptic together.
Previews ignore rate limits and category switches (hearing a muted category is
their point) but obey the master switches and the system state; a note at the top
says when the phone is silent, in Do Not Disturb, has touch vibration off, or has
no vibrator. Everything defaults on at the restrained levels above.

## Debugging

Debug builds log one line per decision to the `ZeronFeedback` tag: what played and
as what (`haptic Select play ViewConstant(constant=26)`, `cue Detent play vol=0.64
rate=1.33 step=5`) or why it did not (`cue Tap skip: another cue just played`,
`haptic Tick skip: rate limited`). `adb shell dumpsys vibrator_manager` lists recent
vibrations with the primitives played. Session events can be driven without an
agent: `adb shell am broadcast -a sh.zeron.android.DEBUG_EVENT -p sh.zeron.android
--es kind done|input|failed [--ez background true]` (`background` takes the
notification branch).

## Regenerating

```sh
python3 scripts/generate-android-sounds.py          # res/raw/fx_*.wav (+ promoted desktop auditions)
python3 scripts/audit-android-sounds.py             # asserts and rewrites android-audit.md
```

Both are standard-library only and deterministic (bit-identical output). The audit
asserts durations (interface cues at most about 120 ms, event cues at most about
600 ms plus the promoted tails), peaks and RMS bands, no clipping, zero first and
last samples with zero-slope fades, DC offset, rising Open and falling Close,
rising ToggleOn and falling ToggleOff, and that interface cues stay 2 to 4 dB
under the desktop chimes. The full table is in [android-audit.md](android-audit.md).

## Verification and its limits

JVM tests (`apps/android/app/src/test/java/sh/zeron/android/feedback`) cover the
gate (switches, system state, silent, DND, throttling, stream cap), the haptic
planner (a plan for every `Haptic` on several simulated devices, strength
scaling), the cue table (every `Cue` has a file), the settings codec and the
event policy with a recording fake (a finished session fires Done once; a
background event becomes a notification and nothing in-app). On the emulator the
log, `dumpsys vibrator_manager` and `dumpsys notification` prove what fires and
how. **The feel of the haptics and the sound quality of the cues need a human on a
phone**: the emulator has no vibration motor you can feel and no audio output.
