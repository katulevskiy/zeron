#!/usr/bin/env python3
"""Zeron Android interface sounds: tiny, near-subliminal, mono, deterministic.

Standard-library-only synthesis in the same "rounded pressure pulse" language
as the desktop notification family (scripts/generate-notification-sounds.py,
scripts/generate-sound-auditions.py): a smooth Ricker-style pressure pulse for
the physical contact, plus a short, damped, harmonic note for the meaning. No
samples, no random without a fixed seed (the one noise swish uses a private
xorshift generator with a constant seed, so output is bit-identical per run).

Output: apps/android/app/src/main/res/raw/fx_<name>.wav, mono, 16-bit PCM,
48 kHz. With --sync-promoted (default) five desktop auditions are also copied
into the same folder (stereo -> mono, level untouched), see PROMOTED below.

TONAL FAMILY
    Everything is built from the C-major pentatonic scale (C D E G A), which
    cannot clash with itself and also matches the desktop cues (C4/D4/E4/G4,
    F4 only in the Appshot). The detent's fixed reference pitch is A5 = 880 Hz,
    a member of the same scale: the app resamples it with SoundPool playback
    rates 0.5..2.0 (440..1760 Hz) along a pentatonic ladder, so it is a pure,
    narrowband sine (no pulse, no harmonics) and pitch-shifting stays natural.

MEANING IS PITCH DIRECTION
    rising = opening / turning on / adding / refreshing
    falling = closing / turning off / removing / failing
    Rising cues are written with a swell (the energy arrives late, at the
    higher pitch) so that they are also brighter; falling cues decay from their
    high point. Lower and heavier means more destructive (Delete is the lowest).

LOUDNESS
    Every interface cue is scaled to the same RMS over its active region
    (first to last sample above -30 dB re peak), default -44.0 dBFS (peaks
    around -30 dBFS). That is about 2 to 4 dB below the desktop done/request/
    attention cues (active RMS -40.3 .. -41.8 dBFS). SoundPool volume cannot
    exceed 1.0, so the source level is the only lever and this is an
    audibility floor on a phone speaker; the cues are also far shorter than the
    desktop ones, so they are perceptually quieter still. The audit enforces
    the matching band and the margin to the desktop cues.

CLICK-FREE
    A raised-sine master fade-in and fade-out forces the first and last sample
    to exactly 0 and a zero slope at both ends.

HAPTIC PAIRING (Feedback.kt Haptic -> Cue)
    Tap          <- Tick / Select   shortest, most neutral
    Select       <- Select          a little warmer/rounder than Tap
    ToggleOn     <- ToggleOn        rising fifth-ish scoop, bright
    ToggleOff    <- ToggleOff       falling, darker, fewer harmonics
    Open         <- Select          rising swell
    Close        <- Select          falling, lower than Open
    Detent       <- Tick            880 Hz pure tick, resampled per step
    Star         <- Pop             bright upward pop with a sparkle
    Unstar       <- Pop             soft downward counterpart
    Pin          <- Pop             firm short thock with a small rise
    Archive      <- Select          soft falling swish + thud
    Delete       <- Heavy           lowest, weightiest falling thud
    Copy         <- Confirm         quick tiny rising double tick
    Error        <- Error           restrained low two-note descent
    Refresh      <- Select          soft two-step rise
"""
import argparse
import hashlib
import math
import struct
import wave
from pathlib import Path

RATE = 48000
TAU = 2 * math.pi
ROOT = Path(__file__).resolve().parents[1]
RAW = ROOT / 'apps/android/app/src/main/res/raw'
AUDITIONS = ROOT / 'docs/sound-design/auditions'

# Active-region definition shared with scripts/audit-android-sounds.py.
ACTIVE_FLOOR_DB = -30.0
TARGET_RMS_DBFS = -44.0
PEAK_LIMIT_DBFS = -3.0

# Pentatonic (C D E G A) pitch table, Hz.
C3, G3 = 130.81, 196.00
E4, C4, G4 = 329.63, 261.63, 392.00
D4, A4 = 293.66, 440.00
C5, D5, E5, G5, A5 = 523.25, 587.33, 659.26, 783.99, 880.00
C6, D6, E6, G6 = 1046.50, 1174.66, 1318.51, 1567.98
A3, D3 = 220.00, 146.83

# name -> desktop audition source (copied, mono, no re-synthesis).
PROMOTED = {
    'fx_send': '01-send.wav',
    'fx_queued': '02-queued.wav',
    'fx_upload_ready': '03-upload-ready.wav',
    'fx_reconnected': '09-reconnected.wav',
    'fx_undo': '10-undo.wav',
}


# ---------------------------------------------------------------- primitives

def samples(ms):
    return round(RATE * ms / 1000.0)


def add_pulse(buf, centre_ms, width_ms, amp):
    """Rounded pressure pulse, (1 - 2x^2) exp(-x^2): zero net area, no DC."""
    centre, width = centre_ms / 1000.0, width_ms / 1000.0
    first = max(0, int((centre - 5 * width) * RATE))
    last = min(len(buf), int((centre + 5 * width) * RATE) + 1)
    for i in range(first, last):
        x = (i / RATE - centre) / width
        buf[i] += amp * (1 - 2 * x * x) * math.exp(-x * x)


def add_note(buf, start_ms, f0, f1, amp, attack_ms, decay_ms,
             partials=((1, 1.0, 1.0),), glide_ms=0.0):
    """Damped harmonic note whose pitch eases from f0 to f1.

    partials: (ratio, level, damping) like the desktop COLORS; damping scales
    the decay time of that partial. The glide is a smoothstep over glide_ms in
    log-frequency; phase is integrated per sample so there are no jumps.
    """
    start = int(start_ms / 1000.0 * RATE)
    attack = attack_ms / 1000.0
    glide = glide_ms / 1000.0
    decay = decay_ms / 1000.0
    phase = 0.0
    log0, log1 = math.log(f0), math.log(f1)
    for i in range(start, len(buf)):
        u = (i - start) / RATE
        if u > decay * 9:
            break
        if glide > 0:
            s = min(u / glide, 1.0)
            s = s * s * (3 - 2 * s)
            freq = math.exp(log0 + (log1 - log0) * s)
        else:
            freq = f1
        phase += TAU * freq / RATE
        rise = 0.5 - 0.5 * math.cos(math.pi * u / attack) if u < attack else 1.0
        value = 0.0
        for ratio, level, damping in partials:
            value += level * math.exp(-u / (decay * damping)) * math.sin(ratio * phase)
        buf[i] += amp * rise * value


def add_swish(buf, start_ms, length_ms, amp, cutoff_from, cutoff_to, seed=0x2545F491):
    """Soft falling air: xorshift noise through a swept two-stage low-pass."""
    state = seed
    start, length = samples(start_ms), samples(length_ms)
    y1 = y2 = 0.0
    for n in range(length):
        state ^= (state << 13) & 0xFFFFFFFF
        state ^= state >> 17
        state ^= (state << 5) & 0xFFFFFFFF
        noise = state / 2147483648.0 - 1.0
        s = n / length
        cutoff = cutoff_from * (cutoff_to / cutoff_from) ** s
        a = 1 - math.exp(-TAU * cutoff / RATE)
        y1 += a * (noise - y1)
        y2 += a * (y1 - y2)
        bell = math.sin(math.pi * s) ** 2
        if start + n < len(buf):
            buf[start + n] += amp * bell * y2


def master_fade(buf, fade_in_ms, fade_out_ms):
    count = len(buf)
    fi, fo = samples(fade_in_ms), samples(fade_out_ms)
    for i in range(count):
        gain = 1.0
        if i < fi:
            gain = math.sin(math.pi / 2 * i / fi) ** 2
        tail = count - 1 - i
        if tail < fo:
            gain *= math.sin(math.pi / 2 * tail / fo) ** 2
        buf[i] *= gain
    buf[0] = 0.0
    buf[-1] = 0.0


def active_rms(buf):
    peak = max(abs(v) for v in buf)
    floor = peak * 10 ** (ACTIVE_FLOOR_DB / 20)
    hits = [i for i, v in enumerate(buf) if abs(v) >= floor]
    span = buf[hits[0]:hits[-1] + 1]
    return math.sqrt(sum(v * v for v in span) / len(span))


# ---------------------------------------------------------------------- cues
# Each cue: (duration_ms, fade_in_ms, fade_out_ms, builder). Levels inside a
# builder are relative; the whole cue is RMS-normalised afterwards.

def cue_tap(b):
    # Shortest, most neutral: one narrow pulse and a tiny G5 body.
    add_pulse(b, 3.5, 0.40, 1.0)
    add_note(b, 2.5, G5, G5, 0.45, 1.0, 5.0, ((1, 1, 1), (2, .10, .6)))


def cue_select(b):
    # Warmer than Tap: wider (lower) pulse, E5 body with a round 2nd partial.
    add_pulse(b, 4.0, 0.62, 1.0)
    add_note(b, 3.0, E5, E5, 0.70, 1.5, 9.0, ((1, 1, 1), (2, .22, .6)))


def cue_toggle_on(b):
    # Bright rising scoop E5 -> C6, energy arriving late, clear harmonics.
    add_pulse(b, 4.0, 0.34, 0.35)
    add_note(b, 3.0, E5, C6, 1.0, 14.0, 20.0, ((1, 1, 1), (2, .20, .8), (3, .07, .6)), glide_ms=38)


def cue_toggle_off(b):
    # Darker falling counterpart G5 -> D5, fewer harmonics, soft pulse.
    add_pulse(b, 4.0, 0.55, 0.45)
    add_note(b, 3.0, G5, D5, 1.0, 2.0, 15.0, ((1, 1, 1), (2, .05, .6)), glide_ms=34)


def cue_open(b):
    # Airy rising swell D5 -> A5.
    add_note(b, 2.0, D5, A5, 1.0, 34.0, 26.0, ((1, 1, 1), (2, .12, .7)), glide_ms=70)


def cue_close(b):
    # Lower, falling counterpart E5 -> A4 with a soft close-contact pulse.
    add_pulse(b, 4.0, 0.80, 0.25)
    add_note(b, 3.0, E5, A4, 1.0, 4.0, 28.0, ((1, 1, 1), (2, .12, .7)), glide_ms=60)


def cue_detent(b):
    # 880 Hz (A5) reference. Pure sine, 1.5 ms swell, no pulse, no harmonics:
    # narrowband so SoundPool rate 0.5..2.0 transposes it without artefacts.
    add_note(b, 1.0, A5, A5, 1.0, 1.5, 7.0)


def cue_star(b):
    # Bright upward pop: A5 -> E6 scoop, then a G6 sparkle on top.
    add_pulse(b, 4.0, 0.30, 0.30)
    add_note(b, 3.0, A5, E6, 0.8, 12.0, 14.0, ((1, 1, 1), (2, .10, .6)), glide_ms=30)
    add_note(b, 36.0, G6, G6, 1.0, 3.0, 16.0, ((1, 1, 1), (2, .04, .5)))


def cue_unstar(b):
    # Soft downward, no pulse: C6 -> G5.
    add_note(b, 2.0, C6, G5, 1.0, 5.0, 20.0, ((1, 1, 1),), glide_ms=45)


def cue_pin(b):
    # Firm short thock: low rounded pulse and a body that rises E4 -> G4.
    add_pulse(b, 3.5, 0.80, 1.0)
    add_note(b, 2.5, E4, G4, 1.1, 1.5, 14.0, ((1, 1, 1), (2, .55, .7), (3, .18, .5)), glide_ms=22)


def cue_archive(b):
    # Soft falling swish, then a muted thud E4 -> A3.
    add_swish(b, 2.0, 68.0, 0.55, 2400.0, 700.0)
    add_pulse(b, 34.0, 1.00, 0.55)
    add_note(b, 31.0, E4, A3, 1.0, 3.0, 26.0, ((1, 1, 1), (2, .28, .6)), glide_ms=40)


def cue_delete(b):
    # Weighty falling thud G3 -> C3; upper partials keep it audible on a
    # phone speaker while the energy centre stays the lowest of the set.
    add_pulse(b, 5.0, 1.40, 1.0)
    add_note(b, 3.0, G3, C3, 1.4, 3.0, 38.0,
             ((1, 1, 1), (2, .55, .7), (3, .26, .55), (4, .10, .4)), glide_ms=60)


def cue_copy(b):
    # Quick tiny double tick, the second a step up (G5 then C6).
    add_pulse(b, 3.5, 0.30, 0.8)
    add_note(b, 2.5, G5, G5, 0.35, 0.8, 4.0)
    add_pulse(b, 31.5, 0.27, 1.0)
    add_note(b, 30.0, C6, C6, 0.45, 0.8, 4.0)


def cue_error(b):
    # Restrained low two-note descent E4 -> C4, soft attacks, mild 2nd partial.
    add_pulse(b, 4.0, 1.0, 0.25)
    add_note(b, 2.0, E4, E4, 1.0, 6.0, 28.0, ((1, 1, 1), (2, .30, .7), (3, .08, .5)))
    add_pulse(b, 92.0, 1.1, 0.22)
    add_note(b, 88.0, C4, C4, 0.9, 6.0, 34.0, ((1, 1, 1), (2, .30, .7), (3, .06, .5)))


def cue_refresh(b):
    # Soft two-step rise: C5 then E5 (a gentle major third).
    add_pulse(b, 4.0, 0.60, 0.20)
    add_note(b, 2.0, C5, C5, 0.7, 4.0, 18.0, ((1, 1, 1), (2, .10, .6)))
    add_pulse(b, 54.0, 0.55, 0.20)
    add_note(b, 50.0, E5, E5, 1.0, 4.0, 22.0, ((1, 1, 1), (2, .10, .6)))


CUES = {
    'fx_tap': (34, 2.0, 12, cue_tap),
    'fx_select': (58, 2.5, 20, cue_select),
    'fx_toggle_on': (72, 2.5, 22, cue_toggle_on),
    'fx_toggle_off': (66, 2.5, 24, cue_toggle_off),
    'fx_open': (108, 4.0, 32, cue_open),
    'fx_close': (100, 4.0, 34, cue_close),
    'fx_detent': (38, 1.5, 14, cue_detent),
    'fx_star': (108, 2.5, 36, cue_star),
    'fx_unstar': (92, 2.5, 34, cue_unstar),
    'fx_pin': (84, 2.0, 28, cue_pin),
    'fx_archive': (112, 3.0, 36, cue_archive),
    'fx_delete': (124, 3.0, 44, cue_delete),
    'fx_copy': (76, 2.0, 22, cue_copy),
    'fx_error': (176, 4.0, 52, cue_error),
    'fx_refresh': (118, 3.0, 40, cue_refresh),
}


def render(name, target_rms_dbfs):
    duration_ms, fade_in, fade_out, builder = CUES[name]
    buf = [0.0] * samples(duration_ms)
    builder(buf)
    master_fade(buf, fade_in, fade_out)
    gain = 10 ** (target_rms_dbfs / 20) / active_rms(buf)
    pcm = [max(-32768, min(32767, round(v * gain * 32768))) for v in buf]
    pcm[0] = pcm[-1] = 0
    peak_db = 20 * math.log10(max(abs(v) for v in pcm) / 32768)
    assert peak_db <= PEAK_LIMIT_DBFS, f'{name}: peak {peak_db:.1f} dBFS'
    return pcm


def write_mono(path, pcm):
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), 'wb') as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(RATE)
        wav.writeframes(struct.pack(f'<{len(pcm)}h', *pcm))


def promote(out_dir):
    """Copy desktop auditions as mono 16-bit 48 kHz. Level is not changed."""
    for name, source in PROMOTED.items():
        with wave.open(str(AUDITIONS / source), 'rb') as wav:
            channels, width, rate = wav.getnchannels(), wav.getsampwidth(), wav.getframerate()
            frames = wav.readframes(wav.getnframes())
        assert width == 2 and rate == RATE, f'{source}: unexpected format {width * 8} bit {rate} Hz'
        count = len(frames) // (2 * channels)
        values = struct.unpack(f'<{count * channels}h', frames)
        if channels == 1:
            pcm = list(values)
        else:
            pcm = [(sum(values[i * channels:(i + 1) * channels]) + channels // 2) // channels
                   for i in range(count)]
        write_mono(out_dir / f'{name}.wav', pcm)
        print(f'{name}.wav: promoted from {source} ({channels} ch -> mono, {count / RATE * 1000:.0f} ms)')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--out', type=Path, default=RAW, help='Output directory (default: res/raw).')
    parser.add_argument('--rms-dbfs', type=float, default=TARGET_RMS_DBFS,
                        help='Active-region RMS shared by all interface cues.')
    parser.add_argument('--sync-promoted', action=argparse.BooleanOptionalAction, default=True,
                        help='Also copy the promoted desktop auditions (default on).')
    args = parser.parse_args()
    for name in CUES:
        pcm = render(name, args.rms_dbfs)
        path = args.out / f'{name}.wav'
        write_mono(path, pcm)
        peak = 20 * math.log10(max(abs(v) for v in pcm) / 32768)
        digest = hashlib.sha256(path.read_bytes()).hexdigest()[:12]
        print(f'{path.name}: {len(pcm) / RATE * 1000:.0f} ms, peak {peak:.1f} dBFS, sha {digest}')
    if args.sync_promoted:
        promote(args.out)


if __name__ == '__main__':
    main()
