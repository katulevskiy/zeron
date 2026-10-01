#!/usr/bin/env python3
"""Numeric audit of the Android sound set (standard library only).

Reads apps/android/app/src/main/res/raw/fx_*.wav and the three desktop
references crates/ui/assets/sounds/{done,request,attention}.wav, measures
level, ends, DC, spectrum and pitch direction, runs assertions, writes
docs/sound-design/android-audit.md and exits non-zero if any assertion fails.

Definitions
  active region   first..last sample whose magnitude is >= peak - 30 dB
                  (same rule as scripts/generate-android-sounds.py)
  RMS active      RMS over the active region; RMS all is over the whole file
  centroid        magnitude-weighted mean frequency, 20 Hz..20 kHz, of the whole
                  file (the files fade to zero, so no analysis window is used)
  dominant        magnitude peak (parabolic interpolation), 20 Hz and up
  direction       the active region is split where half of its energy has
                  passed; the dominant frequency (and centroid) of each part
                  is measured with a Hann window; direction is the change in
                  semitones, second part vs first part. Positive = rising.
  clean ends      first and last sample are 0 (within 1 LSB) and the three
                  samples next to each end stay within 2 LSB, which with the
                  signal's own slope means the file starts and ends on a
                  zero crossing; "zc" additionally reports whether a sign
                  change (or a zero) occurs in the first/last 8 samples.
Desktop references are stereo; they are analysed as the L/R average.
"""
import math
import struct
import sys
import wave
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RAW = ROOT / 'apps/android/app/src/main/res/raw'
SOUNDS = ROOT / 'crates/ui/assets/sounds'
REPORT = ROOT / 'docs/sound-design/android-audit.md'

ACTIVE_FLOOR_DB = -30.0
INTERFACE = ['tap', 'select', 'toggle_on', 'toggle_off', 'open', 'close', 'detent', 'star',
             'unstar', 'pin', 'archive', 'delete', 'copy', 'error', 'refresh']
PROMOTED = ['send', 'queued', 'upload_ready', 'reconnected', 'undo']
REFERENCES = ['done', 'request', 'attention']
# Maximum duration in ms per interface cue.
MAX_MS = {'tap': 60, 'select': 90, 'toggle_on': 90, 'toggle_off': 90, 'detent': 60,
          'open': 120, 'close': 120, 'star': 120, 'unstar': 120, 'pin': 120, 'copy': 120,
          'archive': 120, 'delete': 130, 'refresh': 130, 'error': 180}
RMS_BAND_DB = 1.5
DESKTOP_MARGIN_DB = 2.0
PEAK_LIMIT_DB = -3.0
SIZE_LIMIT_BYTES = 400 * 1024
DIRECTION_MIN_ST = 0.5


def db(x):
    return 20 * math.log10(x) if x > 0 else -999.0


# --------------------------------------------------------------------- io

def read_wav(path):
    with wave.open(str(path), 'rb') as wav:
        channels, width, rate, count = (wav.getnchannels(), wav.getsampwidth(),
                                        wav.getframerate(), wav.getnframes())
        raw = wav.readframes(count)
    assert width == 2, f'{path}: only 16-bit supported'
    values = struct.unpack(f'<{count * channels}h', raw)
    mono = [sum(values[i * channels:(i + 1) * channels]) / channels for i in range(count)]
    return rate, channels, width * 8, mono


# -------------------------------------------------------------------- fft

def fft(values):
    n = len(values)
    j = 0
    data = list(values)
    for i in range(1, n):
        bit = n >> 1
        while j & bit:
            j ^= bit
            bit >>= 1
        j ^= bit
        if i < j:
            data[i], data[j] = data[j], data[i]
    size = 2
    while size <= n:
        step = -2j * math.pi / size
        w_step = complex(math.cos(step.imag), math.sin(step.imag))
        for start in range(0, n, size):
            w = 1 + 0j
            half = size // 2
            for k in range(start, start + half):
                even, odd = data[k], data[k + half] * w
                data[k], data[k + half] = even + odd, even - odd
                w *= w_step
        size <<= 1
    return data


def spectrum(x, rate, hann=False, min_bins=8192):
    n = len(x)
    size = max(min_bins, 1 << (4 * n - 1).bit_length())
    if hann:
        win = [0.5 - 0.5 * math.cos(2 * math.pi * i / max(n - 1, 1)) for i in range(n)]
        padded = [v * w for v, w in zip(x, win)]
    else:
        padded = list(x)
    padded += [0.0] * (size - n)
    mags = [abs(c) for c in fft(padded)[:size // 2 + 1]]
    return rate / size, mags


def centroid_and_dominant(x, rate, hann=False):
    df, mags = spectrum(x, rate, hann)
    lo, hi = max(1, int(20 / df)), min(len(mags) - 1, int(20000 / df))
    total = sum(mags[lo:hi + 1])
    centroid = sum(i * df * m for i, m in enumerate(mags[lo:hi + 1], lo)) / total
    peak = max(range(lo, hi + 1), key=lambda i: mags[i])
    a, b, c = mags[peak - 1], mags[peak], mags[peak + 1]
    denom = a - 2 * b + c
    shift = 0.5 * (a - c) / denom if denom else 0.0
    return centroid, (peak + shift) * df, df, mags


# ---------------------------------------------------------------- measures

def active_span(x):
    peak = max(abs(v) for v in x)
    floor = peak * 10 ** (ACTIVE_FLOOR_DB / 20)
    hits = [i for i, v in enumerate(x) if abs(v) >= floor]
    return hits[0], hits[-1] + 1


def rms(x):
    return math.sqrt(sum(v * v for v in x) / len(x))


def near_zero_crossing(x, at_end):
    edge = x[-8:] if at_end else x[:8]
    if any(abs(v) <= 1 for v in edge):
        return True
    return any(a * b <= 0 for a, b in zip(edge, edge[1:]))


def measure(path):
    rate, channels, bits, x = read_wav(path)
    a, b = active_span(x)
    peak = max(abs(v) for v in x)
    r_all, r_act = rms(x), rms(x[a:b])
    centroid, dominant, df, mags = centroid_and_dominant(x, rate)
    # Equal-energy split of the active region for the direction measure.
    seg = x[a:b]
    energy = sum(v * v for v in seg)
    acc, cut = 0.0, len(seg) // 2
    for i, v in enumerate(seg):
        acc += v * v
        if acc >= energy / 2:
            cut = max(8, min(i, len(seg) - 8))
            break
    c1, d1, _, _ = centroid_and_dominant(seg[:cut], rate, hann=True)
    c2, d2, _, _ = centroid_and_dominant(seg[cut:], rate, hann=True)
    # Power fraction within +-150 Hz of the dominant peak (narrowband check).
    power = [m * m for m in mags]
    lo, hi = max(0, int((dominant - 150) / df)), int((dominant + 150) / df) + 1
    return {
        'name': path.stem, 'rate': rate, 'channels': channels, 'bits': bits,
        'bytes': path.stat().st_size, 'ms': len(x) / rate * 1000,
        'peak_db': db(peak / 32768), 'clipped': any(abs(v) >= 32767 for v in x),
        'rms_all_db': db(r_all / 32768), 'rms_act_db': db(r_act / 32768),
        'active_ms': (b - a) / rate * 1000,
        'crest_db': db(peak / r_all), 'first': int(x[0]), 'last': int(x[-1]),
        'edge_ok': (abs(x[0]) <= 1 and abs(x[-1]) <= 1
                    and max(abs(v) for v in x[:3]) <= 2 and max(abs(v) for v in x[-3:]) <= 2),
        'zc_start': near_zero_crossing(x, False), 'zc_end': near_zero_crossing(x, True),
        'dc': sum(x) / len(x), 'centroid': centroid, 'dominant': dominant,
        'dir_dom_st': 12 * math.log2(d2 / d1), 'dir_cen_st': 12 * math.log2(c2 / c1),
        'dom1': d1, 'dom2': d2,
        'band_fraction': sum(power[lo:hi]) / sum(power[1:]),
    }


# ------------------------------------------------------------------- main

def main():
    results = {}
    for name in INTERFACE + PROMOTED:
        path = RAW / f'fx_{name}.wav'
        if not path.exists():
            print(f'missing {path}', file=sys.stderr)
            return 2
        results[name] = measure(path)
    for name in REFERENCES:
        results[name] = measure(SOUNDS / f'{name}.wav')
    m = results

    checks = []

    def check(label, ok, detail=''):
        checks.append((label, bool(ok), detail))

    fx = INTERFACE + PROMOTED
    for name in fx:
        r = m[name]
        check(f'{name}: mono 16-bit 48 kHz', (r['channels'], r['bits'], r['rate']) == (1, 16, 48000),
              f"{r['channels']} ch, {r['bits']} bit, {r['rate']} Hz")
        check(f'{name}: no clipping', not r['clipped'])
        check(f'{name}: peak <= {PEAK_LIMIT_DB:.0f} dBFS', r['peak_db'] <= PEAK_LIMIT_DB,
              f"{r['peak_db']:.1f}")
        check(f'{name}: clean ends (0 within 1 LSB, near zero crossing)',
              r['edge_ok'] and r['zc_start'] and r['zc_end'], f"first {r['first']}, last {r['last']}")
    for name in INTERFACE:
        r = m[name]
        check(f'{name}: duration <= {MAX_MS[name]} ms', r['ms'] <= MAX_MS[name], f"{r['ms']:.0f} ms")
        check(f'{name}: DC offset < 2% of RMS', abs(r['dc']) < 0.02 * 32768 * 10 ** (r['rms_all_db'] / 20),
              f"{r['dc']:.2f} LSB")

    check('Open centroid > Close centroid', m['open']['centroid'] > m['close']['centroid'],
          f"{m['open']['centroid']:.0f} vs {m['close']['centroid']:.0f} Hz")
    check('ToggleOn centroid > ToggleOff centroid', m['toggle_on']['centroid'] > m['toggle_off']['centroid'],
          f"{m['toggle_on']['centroid']:.0f} vs {m['toggle_off']['centroid']:.0f} Hz")
    check('Star centroid > Unstar centroid', m['star']['centroid'] > m['unstar']['centroid'],
          f"{m['star']['centroid']:.0f} vs {m['unstar']['centroid']:.0f} Hz")
    check('Select warmer (lower centroid) than Tap', m['select']['centroid'] < m['tap']['centroid'],
          f"{m['select']['centroid']:.0f} vs {m['tap']['centroid']:.0f} Hz")
    check('Tap is the shortest interface cue', all(m['tap']['ms'] <= m[n]['ms'] for n in INTERFACE),
          f"{m['tap']['ms']:.0f} ms")
    lowest = min(INTERFACE, key=lambda n: m[n]['centroid'])
    check('Delete has the lowest centroid of the interface set', lowest == 'delete',
          f"lowest is {lowest} ({m[lowest]['centroid']:.0f} Hz); delete {m['delete']['centroid']:.0f} Hz")
    for name, sign in [('toggle_on', 1), ('toggle_off', -1), ('star', 1), ('unstar', -1),
                       ('open', 1), ('close', -1), ('pin', 1), ('refresh', 1), ('error', -1),
                       ('archive', -1), ('delete', -1)]:
        d = m[name]['dir_dom_st']
        check(f"{name} {'rises' if sign > 0 else 'falls'} (dominant, equal-energy halves)",
              sign * d >= DIRECTION_MIN_ST, f"{d:+.1f} st ({m[name]['dom1']:.0f} -> {m[name]['dom2']:.0f} Hz)")
    det = m['detent']
    check('Detent dominant frequency is 880 Hz (+-10)', abs(det['dominant'] - 880) <= 10,
          f"{det['dominant']:.1f} Hz")
    check('Detent is narrowband (>= 90% of power within +-150 Hz)', det['band_fraction'] >= 0.9,
          f"{det['band_fraction'] * 100:.1f}%")

    levels = sorted(m[n]['rms_act_db'] for n in INTERFACE)
    median = levels[len(levels) // 2]
    check(f'Interface active RMS within +-{RMS_BAND_DB} dB of the set median',
          all(abs(m[n]['rms_act_db'] - median) <= RMS_BAND_DB for n in INTERFACE),
          f'median {median:.2f} dBFS, spread {levels[-1] - levels[0]:.2f} dB')
    for ref in REFERENCES:
        margin = min(m[ref]['rms_act_db'] - m[n]['rms_act_db'] for n in INTERFACE)
        check(f'Every interface cue >= {DESKTOP_MARGIN_DB:g} dB quieter (active RMS) than desktop {ref}',
              margin >= DESKTOP_MARGIN_DB, f'smallest margin {margin:.1f} dB')
    total = sum(m[n]['bytes'] for n in fx)
    check('fx_*.wav total size < 400 KB', total < SIZE_LIMIT_BYTES, f'{total / 1024:.0f} KB')

    # ------------------------------------------------------------- report
    out = ['<!-- generated by scripts/audit-android-sounds.py, do not edit -->', '',
           '# Android sound audit', '',
           'Generated by `scripts/audit-android-sounds.py`, do not edit. Regenerate with',
           '`python3 scripts/generate-android-sounds.py && python3 scripts/audit-android-sounds.py`.',
           '', f'Result: **{"PASS" if all(ok for _, ok, _ in checks) else "FAIL"}** '
           f'({sum(ok for _, ok, _ in checks)}/{len(checks)} assertions).', '',
           'Definitions: active region = first to last sample within 30 dB of the peak; centroid = '
           'magnitude-weighted mean frequency of the whole file; dir = change of dominant frequency, '
           'second vs first equal-energy half of the active region, in semitones (positive rises). '
           'Desktop references are stereo and analysed as the L/R average.', '',
           '## Levels, ends and format', '',
           '| file | set | fmt | ms | active ms | peak dBFS | RMS all | RMS active | crest dB | first | last | zc start/end | DC (LSB) | bytes |',
           '|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---:|']
    groups = [(INTERFACE, 'interface', 'fx_'), (PROMOTED, 'promoted', 'fx_'), (REFERENCES, 'desktop ref', '')]
    for names, label, prefix in groups:
        for n in names:
            r = m[n]
            fmt = f"{r['channels']}ch/{r['bits']}b/{r['rate'] // 1000}k"
            out.append(f"| {prefix}{n} | {label} | {fmt} | {r['ms']:.1f} | {r['active_ms']:.1f} | "
                       f"{r['peak_db']:.1f} | {r['rms_all_db']:.1f} | {r['rms_act_db']:.1f} | "
                       f"{r['crest_db']:.1f} | {r['first']} | {r['last']} | "
                       f"{'yes' if r['zc_start'] else 'NO'}/{'yes' if r['zc_end'] else 'NO'} | "
                       f"{r['dc']:+.2f} | {r['bytes']} |")
    out += ['', '## Spectrum and pitch direction', '',
            '| file | centroid Hz | dominant Hz | first half Hz | second half Hz | dir (dominant) st | dir (centroid) st |',
            '|---|---:|---:|---:|---:|---:|---:|']
    for names, _, prefix in groups:
        for n in names:
            r = m[n]
            out.append(f"| {prefix}{n} | {r['centroid']:.0f} | {r['dominant']:.0f} | {r['dom1']:.0f} | "
                       f"{r['dom2']:.0f} | {r['dir_dom_st']:+.1f} | {r['dir_cen_st']:+.1f} |")
    ref_act = {ref: m[ref]['rms_act_db'] for ref in REFERENCES}
    mean_if = sum(m[n]['rms_act_db'] for n in INTERFACE) / len(INTERFACE)
    out += ['', '## Loudness relative to the desktop cues', '',
            f'Interface set mean active RMS: {mean_if:.2f} dBFS (spread '
            f'{levels[-1] - levels[0]:.2f} dB).', '',
            '| desktop cue | RMS active | RMS all | interface is quieter by (active) |',
            '|---|---:|---:|---:|']
    for ref in REFERENCES:
        out.append(f"| {ref} | {ref_act[ref]:.1f} | {m[ref]['rms_all_db']:.1f} | {ref_act[ref] - mean_if:.1f} dB |")
    mean_p = sum(m[n]['rms_act_db'] for n in PROMOTED) / len(PROMOTED)
    out += ['', f'Why only {DESKTOP_MARGIN_DB:g} dB: the margin to the desktop cues is an audibility floor, '
            'not a target. SoundPool volume cannot exceed 1.0, so the source level is the only lever, '
            'and at 6 dB or more below the desktop cues the interface cues peaked near -36 dBFS, which '
            'is inaudible on a phone speaker. The interface cues are also far shorter than the desktop '
            'cues (tens of milliseconds against several hundred), so at equal RMS they are perceptually '
            'quieter still. The rule is kept as an assertion, measured on active-region RMS.', '',
            f'Promoted desktop auditions (levels untouched) average {mean_p:.1f} dBFS active RMS, '
            f'{mean_p - mean_if:+.1f} dB relative to the interface set.', '',
            'Scope: peak, clean-end, format and clipping assertions apply to every `fx_` file; the '
            'duration, direction, centroid-order and desktop-margin assertions apply to the 15 '
            'synthesised interface cues only, because the promoted desktop cues are intentionally '
            'at desktop level and multi-click.', '',
            '## Assertions', '', '| result | assertion | measured |', '|---|---|---|']
    for label, ok, detail in checks:
        out.append(f"| {'pass' if ok else '**FAIL**'} | {label} | {detail} |")
    REPORT.write_text('\n'.join(out) + '\n')

    failed = [c for c in checks if not c[1]]
    for label, ok, detail in checks:
        if not ok:
            print(f'FAIL: {label} [{detail}]')
    print(f'{len(checks) - len(failed)}/{len(checks)} assertions passed; wrote {REPORT.relative_to(ROOT)}')
    for n in INTERFACE + PROMOTED + REFERENCES:
        r = m[n]
        print(f"{n:>13}: {r['ms']:6.1f} ms  peak {r['peak_db']:6.1f}  rms {r['rms_act_db']:6.1f}  "
              f"cent {r['centroid']:6.0f}  dom {r['dominant']:6.0f}  dir {r['dir_dom_st']:+5.1f}/{r['dir_cen_st']:+5.1f}")
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
