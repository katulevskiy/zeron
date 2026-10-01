# Android: sounds and haptics

The phone's sensory layer: how Zeron for Android feels and sounds. It reuses the
desktop's notification family (`done`, `request`, `attention`, see
[README.md](README.md)) for session events, promotes five of the desktop
auditions, and adds thirty-one small interface cues in the same "rounded pressure
pulse" language (fifteen for the interface itself, five for thinking power and fast
mode, eleven provider motifs). Code: `apps/android/app/src/main/java/sh/zeron/android/feedback/`.
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
| Sounds | Needs the **Sounds** master, the category switch, a normal ringer (silent and vibrate mute everything), no total-silence / alarms-only Do Not Disturb, and a non-zero system sound volume. Interface cues also follow the system's *Touch sounds* setting unless **Play sounds anyway** is on (see "If you hear nothing"); session chimes never do (they are events, not touch feedback). Priority-only Do Not Disturb does not block them (they are sonification, like keyboard clicks). |
| No audio focus | Cues are `USAGE_ASSISTANCE_SONIFICATION` / `CONTENT_TYPE_SONIFICATION` and never take focus, so music keeps playing and nothing ducks. |
| Rate limits | The same cue never stacks inside its own gap (60 to 400 ms by cue); any two cues of equal or lower priority keep 20 ms apart; at most 4 concurrent streams (a more important cue still gets in); the same haptic is coalesced (70 ms for ticks up to 400 ms for alerts); at most 12 light haptics per second; any alert cuts through light ticks. |
| Order | Wherever a sound and a haptic belong to the same moment, the **sound is issued first**, then the haptic, from the same synchronous call (`Feedback.both`, `Feedback.play`, the default tap, previews). See Latency. |
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
| `EffortStep` (level 0..1) | a thinking-power detent | none (platform constants are too faint) | CLICK 0.70 to 1.00; from level 0.2 a THUD 0.25 to 1.00 @10 ms; from 0.75 a second CLICK 1.0 @22 ms. Without THUD: two CLICKs; without CLICK strength: two TICKs | `CLICK` below level 0.4, `HEAVY_CLICK` above; waveform 10 ms @150-255, gap, 10-38 ms @110-255 |
| `Stretch` (level 0..1) | the effort thumb pulled past the end | none | TICK 0.25 to 0.70 (firmer the further it is pulled); from level 0.6 a LOW_TICK drag 0.35 to 0.80 @12 ms | `EFFECT_TICK`, 8 ms @40-150 |
| `Rebound` | the thumb snaps back | designed | THUD 0.9, LOW_TICK 0.35 @30 ms (a firm thump that decays at once) | `HEAVY_CLICK`, 22 ms @230, 12 ms @100, 10 ms @40 |
| `Surge` | the highest power chosen | designed | five rising TICKs (0.20, 0.30, 0.42, 0.56, 0.70, gaps 60, 50, 40, 32 ms), CLICK 0.9 @24, THUD 1.0 @8: about 250 ms, a crescendo with a hard crack | without THUD a CLICK 1.0 closes; clicks only: a CLICK crescendo; waveform 28 ms bursts at 40, 70, 110, 160, 210 then 40 ms @255 |
| `Zip` | the lowest power chosen | designed | TICK 0.9, TICK 0.9 @28 ms (very short, sharp double tick) | CLICK 0.5 twice; waveform 6 ms @200, 18 ms gap, 6 ms @200 |
| `Lightning` | fast mode on | designed | TICK 0.45, TICK 0.80 @31, LOW_TICK 0.35 @17, TICK 0.90 @44, CLICK 1.0 @52: uneven strength and spacing, about 180 ms, ends in a firm crack | five CLICKs of the same shape; waveform of irregular 5-8 ms bursts ending in 24 ms @255 |
| `RailTick` | landing on a provider on the rail | `SEGMENT_FREQUENT_TICK` (`CLOCK_TICK`) | LOW_TICK 0.3 (lighter than `Tick`) | 6 ms @28 |

`EffortStep` and `Stretch` take a level: `Feedback.haptic(h, level)` with `level`
from 0 to 1 (position on the effort scale, distance pulled). `AndroidFeedback`
passes it to `HapticTable.spec(h, level)`; every other haptic ignores it. At its
lightest (level 0) `EffortStep` is already a CLICK 0.70 against the old `Select`
TICK 0.70 and `Tick` LOW_TICK 0.5, and at the top it is a THUD 1.0 under a full
double CLICK: clearly the strongest detent in the set, as asked.

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
committed. The three session chimes exist twice: `fx_done`, `fx_request`,
`fx_attention` are still copied byte-for-byte from `crates/ui/assets/sounds` by the
Gradle `genSounds` task and are what the notification channels play; the in-app
cues use `fx_chime_done` / `_request` / `_attention`, the same sounds as mono,
leading silence trimmed, plus the slider headroom (+6 dB, see Volume). The five
promoted cues come from `docs/sound-design/auditions/` (stereo to mono, trimmed,
+6 dB). `silence_keepalive.wav` is not a cue (see Latency).
Category = the in-app switch that governs it. "Trigger" lists every place the
cue is wired.

| Cue | Source | ms | Category | Paired haptic | Trigger(s) |
| --- | --- | ---: | --- | --- | --- |
| `Done` | in-app copy of desktop `done.wav` | 513 | Completion | `Success` | a session's turn finished while Zeron is open |
| `Request` | in-app copy of `request.wav` | 593 | Input required | `Attention` | a session is waiting for an answer or approval |
| `Attention` | in-app copy of `attention.wav` | 644 | Errors | `Error` / `Attention` | a session failed; connection lost while a turn runs |
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
| `Surge` | generated | 490 | Interface | `Surge` | the highest thinking power chosen. A rising swell G4 to C6 with a detuned second voice for shimmer, a pentatonic run of sparkles climbing with it (E5 G5 A5 C6 D6 E6) and a bright C-major bloom (C6 E6 G6 C7) at the top. Quiet start, loudest at about 350 ms |
| `Zip` | generated | 85 | Interface | `Zip` | the lowest thinking power chosen. A fast airy streak of band-passed noise falling from 7.5 to 1.8 kHz with a thin pure zing riding it, G6 to C6. Falling = light |
| `Rebound` | generated | 121 | Interface | `Rebound` | the effort thumb snapped back. A soft elastic boing: G4 whose pitch overshoots 55% and wobbles at 26 Hz while it decays, with a low pulse under the hit |
| `FastOn` | generated | 195 | Interface (trim 0.7) | `Lightning` | fast mode on. Eight tiny signed pulses at uneven times (the crackle, centroid above 2 kHz), a bright G6 to G7 zap with a C6 to C7 shadow, and a small C7 + G6 bloom. Quiet by design |
| `FastOff` | generated | 100 | Interface (trim 0.8) | `ToggleOff` | fast mode off. A soft tick, then the charge draining: G6 falling to C5 over 80 ms, fast decay |
| `ProviderClaude` | generated | 115 | Interface | `RailTick` | warm two-note rising pair, E5 then A5, rounded harmonics, soft attack |
| `ProviderCodex` | generated | 83 | Interface | `RailTick` | crisp bracket-like double tick: two identical hollow clicks (odd harmonics) on D6, 41 ms apart |
| `ProviderCursor` | generated | 108 | Interface | `RailTick` | one glassy blip, G6 bending up to A6, inharmonic partials (2.76, 5.4) |
| `ProviderDevin` | generated | 116 | Interface | `RailTick` | soft pad-like minor third, A4 + C5 over an A3 undertone, slow 22 ms bloom |
| `ProviderGrok` | generated | 116 | Interface | `RailTick` | bright quick fifth, C5 then G5, rich upper harmonics, then three sparkles (A6, C7, E7) |
| `ProviderHermes` | generated | 94 | Interface | `RailTick` | fast flutter up: seven steps of the scale C5 to D6, 9 ms apart, with a breath of air |
| `ProviderPi` | generated | 110 | Interface | `RailTick` | three-note tiny arpeggio on the digits 3-1-4 of pi: E5, C5, G5 (third, root, fourth degree) |
| `ProviderOpenCode` | generated | 117 | Interface | `RailTick` | open, hollow tone: an open fifth D5 + A5 in odd harmonics only, like a wooden pipe |
| `ProviderAntigravity` | generated | 116 | Interface | `RailTick` | floaty upward glide C5 to C6 with a slow wobble, and a quieter echo (G5 to G6) 32 ms later |
| `ProviderFavorites` | generated | 116 | Interface | `RailTick` | twinkle: four bell tones (C7, G6, C7, E7) falling in loudness, inharmonic overtones |
| `ProviderOther` | generated | 59 | Interface | `RailTick` | neutral soft pop: a broad rounded pulse with a short low A4 body, no pitch story |

The provider cues are told apart by structure, not just by pitch: the audit builds
a fingerprint of each (12 spectral bands, a 24-slice loudness envelope, an 8-slice
pitch contour) and asserts that no two are closer than a threshold
(`android-audit.md`, "Provider cues are told apart"). `Cue.forProvider(harness)`
maps a harness id to its motif; anything unknown plays `ProviderOther`.

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

Fast mode's lightning **strikes once**, when fast is switched on: one bolt with forks,
a bright flash, two restrikes and a bloom that fades out over 0.9 s
(`LightningFx.LIFE_SECONDS`), starting on the same frame the `Lightning` haptic and
`FastOn` cue fire. After the fade nothing is drawn or animated (no redraw loop, no
timer); switching fast off and on strikes again, and opening the picker with fast
already on shows nothing. With reduced motion it is one still frame at the flash's
peak for 450 ms. It sits at the bottom of the card and never takes touches.

The effort bar has **friction**. The thumb does not track the finger: each level is a
magnetic well (`EffortTuning.WELL_FRACTION = 0.38` of a step each side of the level)
that the finger has to pull out of before the thumb leaves, after which it springs to
the next level (a staircase of smoothsteps, `EffortDrag.target`). The finger itself is
speed-limited to `MAX_STEPS_PER_SECOND = 7.5` (one level per 133 ms), so a quick flick
moves one or two levels, never from Low to Ultrathink, while a deliberate drag across
the bar (1 to 2 s) still gets there. A release carries the thumb at most half a step
(`RELEASE_CARRY_STEPS`) and springs it to the nearest level; there is no fling. A tap on
the track jumps straight to its level. The thumb is a spring (`FOLLOW_*` while dragging,
`SETTLE_*` into a level, `REBOUND_*` after a stretch), and it is the single state the
fill, dots and halo derive from. `EffortStep` fires when the thumb itself crosses over
to the next level, so the haptic lands with the spring snapping in. The end rubber band
starts only once the finger is past the last level's well. Measured on an emulator with
`adb shell input swipe` and the `ZeronFeedback` log (Low to Ultrathink is six steps):
a 100 ms swipe moves one level, 150 ms one or two, 250 ms two, 500 ms four, 1000 ms
and 1500 ms the whole bar.

Stretch past an end (frames: `docs/media/android/picker-stretch-*-before.png` and
`-after.png`, recorded at animator scale 10): the old bar drew a flat bulge past the
rail and a rectangular, rail-clipped fill whose end was cut at the un-stretched
position, so it showed a lighter coloured block past the thumb and then, as the rebound
spring swung inward, a square-ended fill with the grey track end exposed. The fill is
now a capsule whose right end sits between the thumb's centre and its far edge
(`EffortGeometry.fillRight`), derived from the one thumb position, so it stays round
and under the thumb through stretch and rebound.
Not every vocabulary entry is wired by this layer: `Star`, `Pop` for favorites and
the model chip are used by the model picker, `Press` is reserved.

## Volume

The slider is 0 to 100% in ten steps and **starts at 50%**. 50% is exactly the
loudness the app always had at its old maximum; 100% is twice as loud (+6 dB). The
curve, `FeedbackSettings.gainFor(slider)`: `v = 2 * slider`; for `v <= 1` the gain is
`v^2` (the curve the app always used, so 25% reads about -12 dB), above that it is
`2^(v - 1)`, equal dB steps up to gain 2.0 at 100%. Both pieces meet at gain 1 with no
jump.

`SoundPool` volume cannot exceed 1.0, so the headroom is in the files: every `fx_*`
asset is generated 6.02 dB hotter than before (`BOOST_DB` in the generator: active
RMS -38 dBFS, peaks about -24 dBFS, always under -3 dBFS, no clipping, checked by the
audit) and the app plays a cue at `gain * trim / ASSET_BOOST` (`CueTable.volume`).
At the default that is half volume of a file twice as hot as before: the same sound
pressure as yesterday. At 100% it is full volume: twice. The in-app session chimes
carry the same boost; the notification channels still play the desktop originals, so
background alerts did not change.

**Migration.** Settings written by the old layout stored the volume on the old scale
(100% = today's 50%). `FeedbackSettingsCodec.migrate` runs once, keyed on
`feedback.prefs_version` (absent = 1), and halves a stored volume, so a user who had
chosen v keeps the gain `v^2` they were hearing. A fresh install has no stored
volume, is just stamped, and gets 50%. Unit tests cover the mapping, the migration
for stored values 0 to 1, idempotence and malformed values.

## Latency

Reports: sounds are "slightly delayed". The path from finger to ear, and what each
part costs, was traced through `AndroidFeedback`, `SoundBank` and `FeedbackGate`
(the emulator has no audio output, so the costs below are from the platform's
documented behaviour and the code, not a timed capture; the log and the unit tests
prove the changes, only a phone proves the milliseconds).

| Stage | Before | Now |
| --- | --- | --- |
| Default tap | `defaultTap` waits `ClaimTracker.DEFER_MS` = 40 ms after release (so an explicit cue can claim the moment), after the release interaction's own coroutine hop | unchanged on purpose (the wait is what keeps a Tap from stacking on an Open); it is the largest remaining software term, about 2.4 frames at 60 Hz. Explicit cues (`feedbackClickable`, `toggleAction`, `both`) already fire inside the click handler, synchronously |
| Order | haptic first, then sound: the vibrator service is a binder call (about 1 to 3 ms, sometimes tens when the service is busy), and a late sound is the one a person hears as late | sound first everywhere (`both`, `play`, `defaultTap`, `preview`) |
| Gate | per event: `PowerManager.isInteractive`, `AudioManager.getRingerMode` and `getStreamVolume`, `NotificationManager.currentInterruptionFilter`, two `Settings.System` reads, `hasVibrator`: five binder calls and two settings reads on the UI thread | one `SystemSnapshot` read per 2 s (`TtlValue`), dropped at once by the system's own broadcasts (ringer, volume, interruption filter, screen on/off) and content observers on the two touch settings, registered only while foregrounded. `hasVibrator` is read once |
| Output standby | after a few seconds with no stream the audio output goes to standby, and waking it takes tens of ms on many phones (more over Bluetooth): the first sound after a pause came late | a silent looped stream (`silence_keepalive.wav`) keeps the mixer running; started on a finger-down anywhere (`MainActivity.dispatchTouchEvent` to `AndroidFeedback.onTouchDown`, so it is up before the click that sounds), stopped 12 s after the last touch or when the app leaves the foreground (`WarmPolicy`) |
| First play per stream | `SoundPool` creates a stream's `AudioTrack` at its first `play`: a few ms the first four cues paid | after the last sample loads, four silent (-60 dB) plays of the shortest cue create all four tracks (`SoundBank.prime`). The pool has one stream more than the gate allows, for the keep-alive loop |
| Sample rate | assets are 48 kHz; a mixer at another rate must resample every cue and loses the fast path | `AudioManager.PROPERTY_OUTPUT_SAMPLE_RATE` is read at start (logged: `outputRate=`); at 48 kHz (every current phone) nothing changes; otherwise each asset is resampled once to the output rate with a Catmull-Rom interpolator into `cacheDir/sounds-<rate>-<install time>` and loaded from there |
| Attributes | `USAGE_ASSISTANCE_SONIFICATION` only | plus `FLAG_LOW_LATENCY` (deprecated since 29, still read by the audio policy, which then selects the fast output; `SoundPool` has no performance-mode API) |
| Leading silence in the files | 1.1 to 8.2 ms before the sound reaches -40 dB re peak (Archive 8.2; the promoted desktop cues 7.3 to 7.5; Open 3.3), measured by the audit | every file is trimmed to a 0.25 ms lead-in with a click-free fade from exact zero; the audit and `LatencyTest` assert onset within 1 ms for every `fx_` file. The desktop `fx_done`, `fx_request`, `fx_attention` keep their 7 ms (notification sounds; the in-app copies are trimmed) |
| Loading | `onLoadComplete` was tracked; a cue that was not ready yet was dropped (`NotLoaded`) | unchanged: dropping is right (a late sound is worse than none); loading is one background thread at start |

What is **not** fixed, and cannot be in software: the Bluetooth link's own delay,
phone-specific audio HAL buffering (a fast track is typically 10 to 20 ms), and the
`Detent` ladder, which plays at a playback rate other than 1.0 (so the mixer
resamples it; a pre-rendered set per step would avoid that at the cost of five
more files).

## If you hear nothing

Zeron's sounds are played by the app itself as *interface sonification*, the same
channel the keyboard clicks use. They follow your phone's **system sound volume**
(the "Ring / System" slider, not Media), silent and vibrate mode, and Do Not Disturb
set to total silence or alarms only. Check, in this order:

1. Settings, Sounds & haptics: **Sounds** and **Interface sounds** are on.
2. The phone is not on silent or vibrate, and the system sound volume is above zero.
3. A banner at the top of Sounds & haptics explains what the phone is doing to
   Zeron. If it says **Touch sounds are off in your phone's settings**, tap it:
   - **Open sound settings** goes to the phone's sound settings. Turn on *Touch
     sounds* (Pixel and most phones: Settings, Sound & vibration, Touch sounds;
     Samsung: Settings, Sounds and vibration, System sound/vibration control, Touch
     sounds). For vibration also check Vibration & haptics, Touch feedback.
   - **Play sounds anyway** makes Zeron ignore that one phone setting for its
     interface sounds. This is safe because the cues do not go through Android's
     touch-sound path (`playSoundEffect`) at all: the setting does not technically
     stop them, Zeron only honours it by default as a courtesy. Silent mode, Do Not
     Disturb, volume zero and the in-app switches still apply. Session chimes
     never depended on it.
4. Haptics have their own line: *Touch vibration is turned off in system settings*
   means the phone's Touch feedback is off, which Zeron honours for haptics.

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
ten-step **Volume** (default 50%, 100% is twice as loud, see Volume; sits a small
gap under the Interface sounds row), the **Session
sounds** master with independent **Completion**, **Input required** and **Errors**
(mirroring the desktop's Settings, Notifications), **Background alerts**
(notification permission), **Haptics** with **Strength** (Subtle / Standard /
Strong) and a **Try them** list that plays the real cue and haptic together.
Previews ignore rate limits and category switches (hearing a muted category is
their point) but obey the master switches and the system state; a note at the top
says when the phone is silent, in Do Not Disturb, has touch vibration off, or has
no vibrator. The touch-sounds line is tappable and opens a sheet with how to turn the
phone setting on, **Open sound settings** (`ACTION_SOUND_SETTINGS`, falling back to
`ACTION_SETTINGS`) and **Play sounds anyway** (see "If you hear nothing"); the note is
read again whenever the page resumes, so it clears itself after you fix the setting.
Everything defaults on at the restrained levels above.

## Debugging

Debug builds log one line per decision to the `ZeronFeedback` tag: what played and
as what (`haptic Select play ViewConstant(constant=26)`, `cue Detent play vol=0.64
rate=1.33 step=5`) or why it did not (`cue Tap skip: another cue just played`,
`haptic Tick skip: rate limited`). `adb shell dumpsys vibrator_manager` lists recent
vibrations with the primitives played. Session events can be driven without an
agent: `adb shell am broadcast -a sh.zeron.android.DEBUG_EVENT -p sh.zeron.android
--es kind done|input|failed [--ez background true]` (`background` takes the
notification branch). Any vocabulary entry can be fired directly: `--es kind haptic
--es name Surge [--ef level 0.75]` or `--es kind cue --es name ProviderClaude`. The
`ready:` line at start reports `outputRate=`, `assetRate=` and whether assets had to
be resampled.

## Regenerating

```sh
python3 scripts/generate-android-sounds.py          # res/raw/fx_*.wav (+ promoted desktop auditions)
python3 scripts/audit-android-sounds.py             # asserts and rewrites android-audit.md
```

Both are standard-library only and deterministic (bit-identical output). The audit
asserts durations (interface cues at most about 120 ms, event cues at most about
600 ms plus the promoted tails), peaks and RMS bands, no clipping, zero first and
last samples with zero-slope fades, DC offset, rising Open and falling Close,
rising ToggleOn and falling ToggleOff, a rising Surge and falling Zip / FastOff,
onset within 1 ms of the file start for every `fx_` file, that the in-app chimes are
the desktop chimes plus the 6.02 dB headroom, that the provider cues are
structurally distinct, and that interface cues stay 2 to 4 dB under the chimes. The full table is in [android-audit.md](android-audit.md).

## Verification and its limits

JVM tests (`apps/android/app/src/test/java/sh/zeron/android/feedback`) cover the
gate (switches, system state, silent, DND, throttling, stream cap, the "play anyway"
override, reads per decision), the haptic planner (a plan for every `Haptic` at
several levels on several simulated devices, strength scaling, the round-2 designs),
the volume curve and its migration, the latency pieces (`LatencyTest`: the TTL cache,
output-rate handling, the resampler, the keep-warm policy, every shipped file starting
within 1 ms), the cue table (every `Cue` has its own file), the settings codec and the
event policy with a recording fake (a finished session fires Done once; a
background event becomes a notification and nothing in-app). On the emulator the
log, `dumpsys vibrator_manager` and `dumpsys notification` prove what fires and
how. **The feel of the haptics and the sound quality of the cues need a human on a
phone**: the emulator has no vibration motor you can feel and no audio output.
