#!/usr/bin/env python3
"""
The trailer's soundtrack, made from scratch: python3 soundtrack.py out.wav

It follows the cut in src/Trailer.tsx at 120 beats a minute, so a bar is two
seconds: a quiet opening over the problem, a hit as the mark appears, a
groove under the product scenes that lifts for the montage, the runs and the
agents, a breakdown for what's coming next, and a last hit as the team comes
together into the mark. The interface sounds (clicks, keys, messages
arriving, a chime when something lands) sit on the frames they belong to,
panned to where they happen on screen. The times below are the scenes' own
beats, offset by each scene's start.
"""

import sys

import numpy as np
from scipy import signal
from scipy.io import wavfile

SR = 48000
LENGTH = 90.0
N = int(LENGTH * SR)
STEP = 0.125  # a sixteenth note at 120 beats a minute
rng = np.random.default_rng(20261021)


def S(seconds):
    return int(round(seconds * SR))


def hz(note):
    return 440.0 * 2 ** ((note - 69) / 12)


def times(n):
    return np.arange(n) / SR


def filt(x, kind, freq, order=2):
    return signal.sosfilt(signal.butter(order, freq, btype=kind, fs=SR, output='sos'), x, axis=0)


def env(n, attack, decay):
    t = times(n)
    return (1 - np.exp(-t / max(attack, 1e-4))) * np.exp(-t / decay)


def tail(x, seconds=0.012):
    """Fades out the last few milliseconds, so nothing ends on a click."""
    k = min(len(x), S(seconds))
    ramp = np.linspace(1, 0, k)
    x[-k:] *= ramp if x.ndim == 1 else ramp[:, None]
    return x


def pan_x(x):
    """Where something is on the 1920-wide frame, as a pan."""
    return float(np.clip((x - 960) / 960 * 0.75, -0.8, 0.8))


# Stems, and the reverb and echo sends they feed.
STEMS = {name: np.zeros((N, 2)) for name in ('pad', 'arp', 'bass', 'bell', 'kick', 'drums', 'fx', 'ui')}
VERB = np.zeros((N, 2))
ECHO = np.zeros((N, 2))
KICKS = []


def put(stem, sound, at, gain=1.0, pan=0.0, verb=0.0, echo=0.0):
    """Adds a sound to a stem at a time in seconds. Pan runs from -1 (left) to 1 (right), or is an array per sample."""
    if sound.ndim == 1:
        angle = (np.clip(pan, -1, 1) + 1) * np.pi / 4
        sound = np.stack([sound * np.cos(angle), sound * np.sin(angle)], axis=1) * np.sqrt(2)
    sound = sound * gain
    start = S(at)
    a, b = max(0, start), min(N, start + len(sound))
    if a >= b:
        return
    piece = sound[a - start : b - start]
    STEMS[stem][a:b] += piece
    if verb:
        VERB[a:b] += piece * verb
    if echo:
        ECHO[a:b] += piece * echo


# ---------------------------------------------------------------- instruments


def pad_note(note, n, cutoff):
    """Three detuned, softly filtered saws, spread across the stereo field."""
    t = times(n)
    out = np.zeros((n, 2))
    top = float(np.max(cutoff))
    for cents, pan in ((-8, -0.7), (0, 0.0), (8, 0.7)):
        f = hz(note) * 2 ** (cents / 1200)
        voice = np.zeros(n)
        k = 1
        while f * k < min(15000, top * 2.6):
            fk = f * k
            voice += (1 / k) / (1 + (fk / cutoff) ** 4) * np.sin(2 * np.pi * fk * t + rng.uniform(0, 2 * np.pi))
            k += 1
        angle = (pan + 1) * np.pi / 4
        out[:, 0] += voice * np.cos(angle)
        out[:, 1] += voice * np.sin(angle)
    return out / 3


def pad(notes, at, length, cutoff=(1800, 1800), attack=0.25, release=0.7, gain=1.0):
    n = S(length + release)
    t = times(n)
    shape = 0.5 - 0.5 * np.cos(np.pi * np.clip(t / attack, 0, 1))
    shape *= 0.5 + 0.5 * np.cos(np.pi * np.clip((t - length) / release, 0, 1))
    sweep = np.interp(t, [0, length], cutoff)
    sound = sum(pad_note(note, n, sweep) for note in notes) / np.sqrt(len(notes))
    put('pad', sound * shape[:, None], at, 0.67 * gain, verb=0.3)


def pluck(note, bright=0.6, decay=0.22, length=0.8):
    n = S(length)
    t = times(n)
    f = hz(note)
    index = (0.5 + 2.0 * bright) * np.exp(-t / (0.025 + 0.06 * bright))
    y = np.sin(2 * np.pi * f * t + index * np.sin(2 * np.pi * f * t))
    y += 0.2 * bright * np.sin(4 * np.pi * f * t) * np.exp(-t / 0.04)
    return tail(y * env(n, 0.0015, decay))


def bell(note, decay=1.3, length=3.2, index=1.1):
    n = S(length)
    t = times(n)
    f = hz(note)
    y = np.sin(2 * np.pi * f * t + index * np.exp(-t / 0.3) * np.sin(4 * np.pi * f * t))
    y += 0.25 * np.sin(6 * np.pi * f * t + 0.4) * np.exp(-t / 0.22)
    y += 0.08 * np.sin(2 * np.pi * 4.2 * f * t) * np.exp(-t / 0.08)
    return tail(y * env(n, 0.002, decay))


def bass(note, at, length, gain=1.0, attack=0.006, release=0.06):
    n = S(length + release)
    t = times(n)
    f = hz(note)
    y = np.sin(2 * np.pi * f * t) + 0.35 * np.sin(4 * np.pi * f * t) + 0.12 * np.sin(6 * np.pi * f * t)
    if f / 2 >= 36:
        y += 0.45 * np.sin(np.pi * f * t)
    y = np.tanh(1.7 * y) / np.tanh(1.7)
    shape = np.clip(t / attack, 0, 1) * (0.78 + 0.22 * np.exp(-t / 0.1))
    shape *= 0.5 + 0.5 * np.cos(np.pi * np.clip((t - length) / release, 0, 1))
    put('bass', y * shape, at, 0.34 * gain)


def kick_sound():
    n = S(0.5)
    t = times(n)
    f = 46 + 130 * np.exp(-t / 0.032)
    body = np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t / 0.15)
    click = filt(rng.standard_normal(n), 'highpass', 2500) * np.exp(-t / 0.002) * 0.3
    return tail(np.tanh(1.8 * (body + click)) / np.tanh(1.8))


def clap_sound():
    n = S(0.45)
    t = times(n)
    noise = filt(rng.standard_normal(n), 'bandpass', [900, 7000])
    shape = np.zeros(n)
    for start, level, decay in ((0.0, 1, 0.0045), (0.011, 1, 0.0045), (0.022, 1, 0.0045), (0.03, 0.7, 0.09)):
        shape += np.where(t >= start, level * np.exp(-np.clip(t - start, 0, None) / decay), 0)
    return tail(noise * shape * 0.5)


def snare_sound():
    n = S(0.4)
    t = times(n)
    body = (np.sin(2 * np.pi * 190 * t) + 0.5 * np.sin(2 * np.pi * 330 * t)) * np.exp(-t / 0.045)
    noise = filt(rng.standard_normal(n), 'bandpass', [1200, 9000]) * np.exp(-t / 0.11)
    return tail((0.5 * body + 0.8 * noise) * (1 - np.exp(-t / 0.0008)))


def hat_sound(decay=0.03):
    n = S(min(0.7, decay * 8))
    return tail(filt(rng.standard_normal(n), 'highpass', 7500, 4) * env(n, 0.0005, decay))


def shaker_sound():
    n = S(0.2)
    t = times(n)
    return tail(filt(rng.standard_normal(n), 'bandpass', [4500, 11000]) * (1 - np.exp(-t / 0.01)) * np.exp(-t / 0.035))


def crash_sound(length=3.2, decay=0.95):
    n = S(length)
    noise = filt(rng.standard_normal(n), 'highpass', 3000) + 0.4 * filt(rng.standard_normal(n), 'bandpass', [5000, 9000])
    return tail(noise * env(n, 0.001, decay) * 0.35)


def impact_sound(size=1.0, length=4.5):
    n = S(length)
    t = times(n)
    f = 31 + 58 * np.exp(-t / 0.18)
    boom = np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t / (0.6 + 0.6 * size))
    body = filt(rng.standard_normal(n), 'lowpass', 900) * np.exp(-t / 0.2) * 0.5
    return tail(np.tanh(1.5 * (boom + body)) * (1 - np.exp(-t / 0.001)))


def swept_noise(length, f0, f1, width=0.7, shape=1.0):
    """Noise through a band that moves from f0 to f1, made in the frequency domain."""
    n = S(length)
    noise = rng.standard_normal(n + 4096)
    freqs, frames, spectrum = signal.stft(noise, SR, nperseg=2048, noverlap=1536)
    centre = f0 * (f1 / f0) ** (np.clip(frames / length, 0, 1) ** shape)
    octaves = np.log2(np.maximum(freqs[:, None], 1) / centre[None, :])
    _, y = signal.istft(spectrum * np.exp(-0.5 * (octaves / width) ** 2), SR, nperseg=2048, noverlap=1536)
    y = y[:n]
    return y / (np.std(y) + 1e-9)


def whoosh(length=0.45, f0=500, f1=2600, peak=0.6, width=0.8):
    n = S(length)
    u = times(n) / length
    shape = np.where(u < peak, (u / peak) ** 2, np.clip((1 - u) / (1 - peak), 0, 1) ** 1.6)
    return tail(swept_noise(length, f0, f1, width) * shape)


def riser(length, f0=250, f1=7000, tone=None):
    """Noise that climbs and swells into a downbeat, with an optional rising tone under it."""
    n = S(length)
    u = times(n) / length
    y = swept_noise(length, f0, f1, 0.8, 1.6) * u ** 2.4
    if tone is not None:
        f = hz(tone) * 2 ** (u ** 1.5)
        y += 1.2 * np.sin(2 * np.pi * np.cumsum(f) / SR) * u ** 2
    return tail(y, 0.015)


def pop_sound(freq=900, decay=0.04, rise=0.3):
    n = S(0.22)
    t = times(n)
    f = freq * (1 - rise * np.exp(-t / 0.012))
    phase = 2 * np.pi * np.cumsum(f) / SR
    return tail((np.sin(phase) + 0.12 * np.sin(2 * phase)) * env(n, 0.001, decay))


def click_sound():
    """A pointer press and its release."""
    n = S(0.16)
    t = times(n)
    y = np.zeros(n)
    for start, level, f in ((0.0, 1.0, 2900), (0.085, 0.45, 3400)):
        on = t >= start
        u = np.clip(t - start, 0, None)
        noise = filt(rng.standard_normal(n), 'bandpass', [1800, 9000])
        y += on * level * (0.55 * noise * np.exp(-u / 0.0012) + 0.35 * np.sin(2 * np.pi * f * u) * np.exp(-u / 0.0035) + 0.5 * np.sin(2 * np.pi * 190 * u) * np.exp(-u / 0.009))
    return tail(y)


def key_sound():
    n = S(0.05)
    t = times(n)
    noise = filt(rng.standard_normal(n), 'bandpass', [1000 + rng.uniform(0, 1500), 8000])
    body = np.sin(2 * np.pi * rng.uniform(260, 380) * t) * np.exp(-t / 0.006)
    return tail(0.6 * noise * np.exp(-t / 0.0022) + 0.35 * body)


def scribble_sound(length, strokes):
    """A marker on a whiteboard: a few strokes of soft, grainy noise."""
    n = S(length)
    t = times(n)
    noise = filt(rng.standard_normal(n), 'bandpass', [700, 4200])
    shape = np.zeros(n)
    each = length / strokes
    for i in range(strokes):
        u = np.clip((t - i * each) / each, 0, 1)
        shape = np.maximum(shape, np.where((t >= i * each) & (t <= (i + 1) * each), np.sin(np.pi * u) ** 0.7, 0))
    grain = filt(rng.standard_normal(n), 'lowpass', 35)
    grain = 0.75 + 0.25 * grain / (np.std(grain) + 1e-9)
    return tail(noise * shape * grain)


def thock_sound():
    n = S(0.2)
    t = times(n)
    f = 95 + 70 * np.exp(-t / 0.02)
    y = np.sin(2 * np.pi * np.cumsum(f) / SR) * env(n, 0.001, 0.05)
    y += 0.25 * filt(rng.standard_normal(n), 'bandpass', [1500, 6000]) * np.exp(-t / 0.002)
    return tail(y)


KICK = kick_sound()
CLAPS = [clap_sound() for _ in range(3)]
SNARES = [snare_sound() for _ in range(4)]
HATS = [hat_sound(0.028) for _ in range(4)]
OPENS = [hat_sound(0.12) for _ in range(3)]
SHAKERS = [shaker_sound() for _ in range(4)]
KEYS = [key_sound() for _ in range(12)]
CLICK = click_sound()
THOCK = thock_sound()


def pick(options):
    return options[rng.integers(len(options))]


# ---------------------------------------------------------------- the music

CHORDS = {
    'Fopen': dict(pad=[53, 60, 67, 72], bass=41, arp=[65, 67, 72, 77, 79]),
    'Db': dict(pad=[53, 56, 61, 65, 72], bass=41, arp=[65, 68, 72, 73, 77]),
    'F': dict(pad=[53, 57, 64, 67, 72], bass=41, arp=[65, 69, 72, 76, 79]),
    'C/E': dict(pad=[52, 55, 62, 67, 72], bass=40, arp=[64, 67, 72, 74, 79]),
    'Dm': dict(pad=[50, 53, 60, 64, 69], bass=38, arp=[62, 65, 69, 72, 76]),
    'Bb': dict(pad=[53, 58, 62, 69, 72], bass=34, arp=[62, 65, 70, 72, 77]),
    'F/A': dict(pad=[53, 60, 65, 67, 72], bass=33, arp=[65, 69, 72, 77, 79]),
    'Gm': dict(pad=[53, 58, 62, 65, 69], bass=31, arp=[62, 65, 67, 70, 74]),
    'Csus': dict(pad=[55, 60, 65, 67, 70], bass=36, arp=[65, 67, 70, 72, 77]),
    'C': dict(pad=[55, 60, 64, 67, 70], bass=36, arp=[64, 67, 70, 72, 76]),
}

STYLES = {
    # Plan: a light groove comes in.
    'A1': dict(kick=[0, 8], hat=[2, 6, 10, 14], hat_level=0.45, bass='halves', arp=16, arp_level=0.55, bright=0.45, pad=0.8, cutoff=1700),
    'A2': dict(kick=[0, 4, 8, 12], clap=[4, 12], hat=[2, 6, 10, 14], hat_level=0.6, shaker=0.28, bass='eighths', arp=16, arp_level=0.62, bright=0.55, pad=0.7, cutoff=1900),
    # The team talking: half time, so the chat can be heard.
    'B1': dict(kick=[0, 10], clap=[8], hat=list(range(0, 16, 2)), hat_level=0.35, bass='long', pad=0.72, cutoff=2100),
    'B2': dict(kick=[0, 10], clap=[8], hat=list(range(0, 16, 2)), hat_level=0.4, shaker=0.22, bass='long', arp=8, arp_level=0.45, bright=0.5, pad=0.7, cutoff=2200),
    # The montage and the runs.
    'C': dict(kick=[0, 4, 8, 12], clap=[4, 12], open=[2, 6, 10, 14], open_level=0.5, shaker=0.32, bass='eighths', arp=16, arp_level=0.7, bright=0.75, pad=0.65, cutoff=2400),
    # Any agent: everything, plus the tune.
    'D': dict(kick=[0, 4, 8, 12], clap=[4, 12], hat=list(range(16)), hat_level=0.22, open=[2, 6, 10, 14], open_level=0.55, shaker=0.36, bass='octaves', arp=16, arp_level=0.72, bright=0.85, high=0.3, pad=0.75, cutoff=3000),
    'gap': dict(bass='long', pad=0.7, cutoff=2400),
    # What's next: no drums but the hats, and the arpeggio darker.
    'E': dict(hat=[2, 6, 10, 14], hat_level=0.3, shaker=0.18, bass='long', bass_level=0.6, arp=8, arp_level=0.5, bright=0.3, pad=0.8, cutoff=1500),
}


def bars(start, chords, style, beats=4):
    return [(start + i * beats * 0.5, beats, chord, style) for i, chord in enumerate(chords)]


GROOVE = (
    bars(12, ['F', 'C/E'], 'A1')
    + bars(16, ['Dm', 'Bb', 'F', 'C/E', 'Dm', 'Bb', 'F', 'C/E', 'Dm'], 'A2')
    + bars(34, ['Bb', 'F/A', 'Gm', 'Csus|C', 'Bb'], 'B1')
    + bars(44, ['F/A', 'Gm', 'Csus|C'], 'B2')
    + bars(50, ['F', 'C/E', 'Dm'], 'C')
    + [(56, 2, 'Bb', 'C')]
    + bars(57, ['F', 'C/E', 'Dm', 'Bb', 'Csus|C'], 'C')
    + bars(67, ['F', 'C/E', 'Dm', 'Bb'], 'D')
    + [(75, 2, 'C', 'gap')]
    + bars(76, ['Dm', 'Bb', 'F/A'], 'E')
)

UPDOWN = [0, 1, 2, 3, 4, 3, 2, 1]


def kick(at, level=1.0):
    put('kick', KICK, at, 0.5 * level)
    KICKS.append((at, level))


def play_bar(at, beats, chord, style_name):
    style = STYLES[style_name]
    steps = beats * 4
    names = chord.split('|')
    each = steps // len(names)
    parts = [(i * each, each, name) for i, name in enumerate(names)]

    def chord_at(step):
        return CHORDS[parts[min(step // each, len(parts) - 1)][2]]

    for first, count, name in parts:
        pad(CHORDS[name]['pad'], at + first * STEP, count * STEP, (style['cutoff'],) * 2, attack=0.18, release=0.6, gain=style['pad'])

    for step in style.get('kick', []):
        if step < steps:
            kick(at + step * STEP)
    for step in style.get('clap', []):
        if step < steps:
            put('drums', pick(CLAPS), at + step * STEP, 0.6, verb=0.25)
    for step in style.get('hat', []):
        if step < steps:
            accent = 1.0 if step % 4 == 2 else 0.7
            put('drums', pick(HATS), at + step * STEP, 0.26 * style['hat_level'] * accent * rng.uniform(0.88, 1.1), pan=0.3)
    for step in style.get('open', []):
        if step < steps:
            put('drums', pick(OPENS), at + step * STEP, 0.24 * style['open_level'], pan=0.18, verb=0.08)
    if style.get('shaker'):
        for step in range(steps):
            accent = (0.55, 0.35, 1.0, 0.4)[step % 4]
            put('drums', pick(SHAKERS), at + step * STEP + 0.004, 0.12 * style['shaker'] * accent, pan=-0.35)

    mode = style.get('bass')
    level = style.get('bass_level', 1.0)
    if mode == 'halves':
        for step in (0, 8):
            if step < steps:
                bass(chord_at(step)['bass'], at + step * STEP, 0.85, level)
    elif mode in ('eighths', 'octaves'):
        for step in range(0, steps, 2):
            up = 12 if mode == 'octaves' and step % 4 == 2 else 0
            bass(chord_at(step)['bass'] + up, at + step * STEP, 0.19, level * (1.0 if step % 4 == 0 else 0.85))
    elif mode == 'long':
        for first, count, name in parts:
            bass(CHORDS[name]['bass'], at + first * STEP, count * STEP - 0.04, 0.8 * level, attack=0.03, release=0.12)

    if style.get('arp'):
        stride = 16 // style['arp']
        for step in range(0, steps, stride):
            tones = chord_at(step)['arp']
            note = tones[UPDOWN[(step // stride) % len(UPDOWN)]]
            velocity = 1.0 if step % 4 == 0 else (0.8 if step % 2 == 0 else 0.62)
            gain = 0.42 * style['arp_level'] * velocity
            side = -0.3 if (step // stride) % 2 else 0.3
            put('arp', pluck(note, style['bright']), at + step * STEP, gain, pan=side, verb=0.15, echo=0.32)
            if style.get('high'):
                put('arp', pluck(note + 12, style['bright'], 0.16), at + step * STEP, gain * style['high'], pan=-side, verb=0.2, echo=0.3)


for bar in GROOVE:
    play_bar(*bar)


def fill(at, count, level=1.0):
    """A run of snare sixteenths, getting louder into the next downbeat."""
    for i in range(count):
        put('drums', pick(SNARES), at + i * STEP, 0.5 * level * (0.35 + 0.65 * (i + 1) / count), verb=0.2)


def crash(at, level=1.0):
    put('drums', crash_sound(), at, 0.5 * level, pan=-0.15, verb=0.25)


def reverse_crash(end, length=1.2, level=1.0):
    sound = crash_sound(length + 0.4, length * 0.8)[::-1].copy()
    put('fx', sound, end - len(sound) / SR, 0.42 * level, pan=0.15, verb=0.15)


def impact(at, size=1.0, level=1.0):
    put('fx', impact_sound(size), at, 0.95 * level, verb=0.18)
    # Not a groove kick, so the chord that lands with it isn't pushed down.
    put('kick', KICK, at, 0.7 * level)


for at, count in ((27.5, 4), (33.0, 8), (49.0, 8), (56.0, 8), (66.0, 8)):
    fill(at, count)
for at, level in ((12, 0.5), (20, 0.35), (28, 0.6), (34, 0.6), (50, 0.75), (57, 0.75), (67, 0.95), (76, 0.35)):
    crash(at, level)
for end, length, level in ((12, 1.2, 0.6), (34, 1.0, 0.8), (50, 1.0, 0.8), (57, 1.0, 0.8), (67, 1.5, 1.0), (76, 1.0, 0.7)):
    reverse_crash(end, length, level)
put('fx', riser(2.0, 300, 6000), 10.0, 0.05, verb=0.2)
put('fx', riser(2.0, 250, 7000, tone=60), 48.0, 0.07, verb=0.2)
put('fx', riser(2.0, 250, 8000, tone=60), 65.0, 0.08, verb=0.2)

# The tune, over the agents.
MELODY = [
    (67, [(0, 77, 1), (1, 76, 0.5), (1.5, 72, 0.5), (2, 79, 2)]),
    (69, [(0, 76, 1), (1, 74, 0.5), (1.5, 72, 0.5), (2, 67, 2)]),
    (71, [(0, 74, 1), (1, 72, 0.5), (1.5, 69, 0.5), (2, 76, 2)]),
    (73, [(0, 74, 1), (1, 72, 0.5), (1.5, 70, 0.5), (2, 69, 1), (3, 72, 1)]),
]
for start, notes in MELODY:
    for beat, note, _ in notes:
        put('bell', bell(note, 1.0), start + beat * 0.5, 0.13, pan=0.1, verb=0.4, echo=0.22)

# The opening: a held chord that darkens as the tabs pile up.
pad(CHORDS['Fopen']['pad'], 0.0, 4.4, (650, 1100), attack=2.2, release=0.9, gain=0.5)
bass(29, 0.2, 4.3, 0.3, attack=1.5, release=0.8)
pad(CHORDS['Db']['pad'], 4.4, 2.55, (1000, 1900), attack=0.6, release=0.25, gain=0.7)
bass(29, 4.4, 2.5, 0.55, attack=0.4, release=0.2)

# The mark: a hit, and the chords open out under the name.
pad(CHORDS['F']['pad'], 6.95, 2.05, (2800, 1600), attack=0.03, release=0.8, gain=1.1)
pad(CHORDS['Bb']['pad'], 9.0, 2.0, (1600, 1900), attack=0.5, release=0.7, gain=0.9)
pad(CHORDS['Csus']['pad'], 11.0, 1.0, (1900, 2500), attack=0.4, release=0.5, gain=0.9)
bass(41, 6.95, 2.0, 0.8, attack=0.01, release=0.4)
bass(34, 9.0, 1.95, 0.6, attack=0.2, release=0.15)
bass(36, 11.0, 0.95, 0.6, attack=0.2, release=0.1)
for i in range(32):
    at = 8.0 + i * STEP
    chord = 'F' if at < 9 else ('Bb' if at < 11 else 'Csus')
    k = i / 31
    note = CHORDS[chord]['arp'][UPDOWN[i % 8]]
    put('arp', pluck(note, 0.25 + 0.3 * k), at, 0.42 * (0.2 + 0.45 * k) * (1.0 if i % 4 == 0 else 0.7), pan=-0.3 if i % 2 else 0.3, verb=0.2, echo=0.35)

# The close: the team arrives, comes together, and becomes the mark.
pad(CHORDS['Bb']['pad'], 82.0, 2.3, (1400, 2200), attack=0.7, release=0.5, gain=0.95)
pad(CHORDS['Csus']['pad'], 84.3, 0.94, (2200, 3200), attack=0.3, release=0.15, gain=1.0)
pad(CHORDS['F']['pad'] + [76], 85.24, 2.4, (3400, 1800), attack=0.02, release=2.2, gain=1.25)
bass(34, 82.0, 2.25, 0.6, attack=0.3, release=0.1)
bass(36, 84.3, 0.9, 0.7, attack=0.1, release=0.05)
bass(41, 85.24, 2.6, 0.95, attack=0.005, release=1.6)

# ---------------------------------------------------------------- the picture's sounds


UI_LEVEL = 1.0  # lowered for the busy, quieter stretch of what's next


def sfx(sound, at, gain, x=960, verb=0.12):
    put('ui', sound, at, 2.0 * gain * UI_LEVEL, pan=pan_x(x), verb=verb)


def click(at, x=960, gain=1.0):
    sfx(CLICK, at, 0.2 * gain, x, 0.05)


def keys(start, end, x=960, rate=17, gain=1.0):
    """Typing, at most `rate` keys a second, a little uneven."""
    count = max(1, int((end - start) * rate))
    for i in range(count):
        at = start + (i + rng.uniform(-0.25, 0.25)) * (end - start) / count
        sfx(pick(KEYS), max(start, at), 0.055 * gain * rng.uniform(0.7, 1.0), x, 0.03)


def message(at, note=79, x=1200, gain=1.0):
    """Something new in a conversation: a soft rising pair of notes."""
    sfx(pop_sound(hz(note), 0.05), at, 0.1 * gain, x, 0.2)
    sfx(pop_sound(hz(note + 5), 0.07), at + 0.065, 0.075 * gain, x, 0.2)


def blip(at, x=1200, gain=1.0):
    sfx(pop_sound(1550, 0.028, 0.2), at, 0.07 * gain, x, 0.1)


def note_pop(at, note, x=960, gain=1.0):
    sfx(pluck(note, 0.55, 0.12, 0.5), at, 0.11 * gain, x, 0.18)


def chime(at, notes, x=960, gain=1.0):
    for i, note in enumerate(notes):
        sfx(bell(note, 0.9, 2.4, 0.8), at + i * 0.085, 0.12 * gain, x, 0.45)


def swoosh(at, length=0.4, f0=500, f1=2600, x0=960, x1=960, gain=1.0, peak=0.6):
    n = S(length)
    pans = np.linspace(pan_x(x0), pan_x(x1), n)
    put('ui', whoosh(length, f0, f1, peak), at, 0.11 * gain * UI_LEVEL, pan=pans, verb=0.15)


# 0:00 Your team has agents now. They write code, review pull requests, plan whole projects.
for at, note in ((0.32, 65), (1.75, 69), (2.55, 72), (3.35, 77)):
    put('bell', bell(note, 1.6, 3.5), at, 0.16, pan=0.0, verb=0.5, echo=0.25)
# The tabs fly in, each from where it lands, and the problem lands low.
for i, x in enumerate((310, 775, 1675, 1265, 271, 715, 1665, 1250, 115, 1815)):
    swoosh(4.25 + i * 0.075, 0.32, 700, 2400, x, x, 0.55, peak=0.35)
put('fx', impact_sound(0.4, 3.0), 4.55, 0.35, verb=0.3)
# They're pulled into the middle, and the mark appears.
put('fx', riser(1.05, 300, 7000, tone=53), 5.9, 0.1, verb=0.25)
reverse_crash(6.95, 1.0, 0.9)
swoosh(5.9, 1.05, 2400, 500, 300, 960, 1.3, peak=0.85)
impact(6.95, 1.0)
crash(6.95, 0.8)
for i, note in enumerate((72, 77, 81)):
    put('bell', bell(note, 2.2, 4.0), 6.95 + i * 0.03, 0.12, verb=0.55, echo=0.2)
put('bell', bell(89, 1.2, 2.5, 0.6), 7.7, 0.05, verb=0.6, echo=0.3)

# The team under the name lands from the middle out, a note for each pair.
for i, note in enumerate((70, 72, 74, 77, 79, 82)):
    note_pop(9.0 + i * 0.07, note, 960 + (1 if i % 2 else -1) * 90 * i, 0.5)

# 0:11 Plan: paste a plan, import it, see it as a board, move a card, open the task.
PLAN = 11.6
swoosh(PLAN + 0.9, 0.35, 600, 2200, 1300, 1300, 0.6)
for i in range(18):
    sfx(pick(KEYS), PLAN + 1.15 + i * 0.045, 0.04, 1300, 0.03)
click(PLAN + 2.75, 1500)
for i in range(17):
    sfx(pop_sound(1150 + 25 * i, 0.025, 0.15), PLAN + 3.0 + i * 0.06, 0.03, 1250, 0.05)
click(PLAN + 5.0, 1400)
swoosh(PLAN + 5.05, 0.4, 900, 2600, 960, 960, 0.6)
sfx(pop_sound(520, 0.06, 0.5), PLAN + 6.15, 0.09, 900)
swoosh(PLAN + 6.2, 0.9, 700, 1400, 900, 1080, 0.35, peak=0.5)
sfx(THOCK, PLAN + 7.15, 0.18, 1080)
click(PLAN + 7.85, 760)

# 0:19 The task page, a whiteboard linked to it, and Leo and Muse drawing the plan.
TASK = 19.6
swoosh(TASK + 0.05, 0.45, 2200, 700, 760, 1200, 0.6)
click(TASK + 2.8, 1250)
sfx(pop_sound(1300, 0.03, 0.2), TASK + 2.85, 0.05, 1250)
click(TASK + 3.45, 1250)
note_pop(TASK + 3.6, 84, 1250, 0.8)
swoosh(TASK + 4.15, 0.6, 500, 2000, 1250, 1100, 0.9)
BOARD = TASK + 4.15
for at, length, strokes, x in (
    (0.5, 0.5, 4, 760), (0.85, 0.3, 2, 900), (0.9, 0.35, 3, 1500), (0.95, 0.5, 4, 1010),
    (1.3, 0.3, 2, 1150), (1.4, 0.5, 4, 1260), (1.75, 0.3, 2, 1010), (1.95, 0.5, 4, 1010),
    (2.4, 0.3, 2, 1150), (2.55, 0.5, 4, 1260),
):
    sfx(scribble_sound(length, strokes), BOARD + at, 0.035, x, 0.08)
swoosh(TASK + 8.1, 0.45, 2200, 600, 1200, 960, 0.5)

# 0:28 The planning graph: what's ready, the critical path, a suggestion accepted.
GRAPH = 28.0
for column, note in enumerate((72, 76, 77, 79, 84)):
    note_pop(GRAPH + 0.35 + column * 0.28, note, 200 + column * 260, 0.7)
put('bell', bell(84, 0.8, 2.0, 0.5), GRAPH + 1.7, 0.05, pan=-0.1, verb=0.5)
swoosh(GRAPH + 2.2, 0.6, 900, 3200, 300, 1250, 0.5)
note_pop(GRAPH + 2.7, 81, 1150, 0.8)
click(GRAPH + 3.95, 1150)
chime(GRAPH + 4.0, [77, 84], 1150, 0.8)

# 0:34 #launch: Claude's update, the team talking, Hermes chiming in, then Priya tags Codex and Muse.
TEAM = 34.0
for at, note in ((0.5, 77), (1.7, 79), (2.65, 81), (3.55, 84), (4.85, 79)):
    message(TEAM + at, note, 1250)
for at in (1.25, 2.0, 2.3, 4.4):
    blip(TEAM + at, 1250)
click(TEAM + 5.65, 1250)
sfx(pick(KEYS), TEAM + 5.95, 0.06, 1250)
keys(TEAM + 6.15, TEAM + 6.45, 1250)
note_pop(TEAM + 6.5, 84, 1250, 0.7)
keys(TEAM + 6.55, TEAM + 6.55 + 20 / 32, 1250)
sfx(pick(KEYS), TEAM + 7.3, 0.06, 1250)
keys(TEAM + 7.45, TEAM + 7.7, 1250)
note_pop(TEAM + 7.75, 86, 1250, 0.7)
keys(TEAM + 7.8, TEAM + 7.8 + 47 / 44, 1250)
swoosh(TEAM + 8.95, 0.3, 800, 3000, 1250, 1250, 0.6, peak=0.4)
message(TEAM + 9.0, 84, 1250, 0.9)
sfx(pop_sound(1700, 0.03, 0.2), TEAM + 9.35, 0.05, 1250)
swoosh(TEAM + 9.8, 0.5, 600, 2400, 1900, 1500, 0.8)
sfx(pop_sound(1200, 0.03, 0.2), TEAM + 10.35, 0.05, 1500)
for at, note in ((11.75, 81), (12.2, 84), (13.45, 79)):
    message(TEAM + at, note, 1500)
blip(TEAM + 14.05, 1500)

# 0:49 The rest of the chat: inbox, a DM with Claude, Later, search, pins and threads.
FEATURES = 49.6
TILE_X = (330, 960, 1590, 330, 960, 1590)
for i, note in enumerate((72, 76, 77, 79, 81, 84)):
    note_pop(FEATURES + 0.35 + i * 0.15, note, TILE_X[i], 0.75)
keys(FEATURES + 0.9, FEATURES + 0.9 + 34 / 30, 960, gain=0.8)
keys(FEATURES + 1.15, FEATURES + 1.15 + 6 / 16, 330, gain=0.8)
click(FEATURES + 1.45, 960, 0.8)
click(FEATURES + 1.7, 330)
swoosh(FEATURES + 2.23, 0.3, 800, 3000, 960, 960, 0.5, peak=0.4)
message(FEATURES + 2.25, 84, 960, 0.8)
click(FEATURES + 3.3, 1590, 0.8)
message(FEATURES + 4.03, 81, 960, 0.8)
note_pop(FEATURES + 4.2, 77, 1590, 0.6)

# 0:56 A run: Launch, every step as it happens, and a pull request back.
RUNS = 56.6
click(RUNS + 1.35, 1680)
swoosh(RUNS + 1.38, 0.5, 400, 2600, 1680, 1680, 0.7, peak=0.5)
note_pop(RUNS + 1.45, 77, 1680, 0.7)
note_pop(RUNS + 2.05, 81, 1680, 0.8)
swoosh(RUNS + 2.7, 0.5, 600, 2800, 1300, 1100, 0.7)
for i, (at, note) in enumerate(((0.5, 72), (1.05, 74), (1.65, 76), (2.25, 77))):
    note_pop(RUNS + 2.85 + at, note + 12, 1200, 0.65)
chime(RUNS + 2.85 + 2.95, [81, 86], 1200)
RUN_BEHIND = RUNS + 2.85 + 2.95 + 0.65
swoosh(RUN_BEHIND, 0.5, 2400, 700, 1200, 1280, 0.7)
message(RUN_BEHIND + 0.3, 81, 1280)
sfx(pop_sound(1250, 0.03, 0.2), RUN_BEHIND + 0.6, 0.05, 1280)
blip(RUN_BEHIND + 1.5, 1280)
chime(RUN_BEHIND + 2.15, [79, 84], 1280, 1.1)

# 1:06 Any agent: the roster, and your own one written with the runner SDK.
AGENTS = 66.8
CODE = [
    "import { Runner, runnerSettingsFromEnv } from '@agent-workspace/runner'",
    '',
    'const runner = new Runner({',
    '  ...runnerSettingsFromEnv(),',
    '  async onRequest(request) {',
    "    await request.progress('Reading the repository')",
    '    const answer = await scout.run(request.prompt)',
    '    await request.reply(answer)',
    '  },',
    '})',
    '',
    'runner.listen(8787)',
]
TYPING = AGENTS + 1.45
RUN = TYPING + sum(len(line) + 1 for line in CODE) / 150 + 0.25
ONLINE = RUN + 0.5
for i, note in enumerate((72, 76, 77, 79, 81, 84, 88)):
    note_pop(AGENTS + 0.45 + i * 0.09, note, 1120 if i % 2 == 0 else 1584, 0.6)
swoosh(AGENTS + 1.0, 0.45, 600, 2000, 480, 480, 0.5)
keys(TYPING, RUN - 0.25, 480, rate=19, gain=0.9)
keys(RUN, RUN + 13 / 40, 480, gain=0.9)
sfx(pick(KEYS), RUN + 0.4, 0.09, 480)
chime(ONLINE, [84, 89], 480)
note_pop(ONLINE + 0.15, 88, 1584, 0.9)
swoosh(AGENTS + 8.55, 0.5, 2200, 500, 960, 960, 0.6)

# 1:15 Coming next: docs and sheets to Google, pairing any machine, a built-in agent.
NEXT = 75.6
UI_LEVEL = 0.7
for i, (note, x) in enumerate(((77, 388), (81, 960), (84, 1532))):
    note_pop(NEXT + 0.4 + i * 0.15, note, x, 0.75)
message(NEXT + 1.25, 81, 1532, 0.8)
keys(NEXT + 1.3, NEXT + 1.3 + 40 / 34, 960, gain=0.8)
sfx(pop_sound(1500, 0.03, 0.2), NEXT + 2.78, 0.05, 960)
click(NEXT + 3.35, 388)
message(NEXT + 3.35, 84, 1532, 0.6)
note_pop(NEXT + 3.47, 84, 388, 0.6)
chime(NEXT + 3.55, [81, 86], 960, 0.9)
# Then each tile keeps going: a thumbs up and a reply, the runner taking work, Grok editing its row.
blip(NEXT + 3.9, 1532, 0.8)
note_pop(NEXT + 4.0, 81, 960, 0.5)
sfx(pop_sound(1400, 0.03, 0.2), NEXT + 4.3, 0.04, 388)
message(NEXT + 4.35, 79, 1532, 0.6)
keys(NEXT + 4.75, NEXT + 4.75 + 9 / 16, 388, gain=0.6)
note_pop(NEXT + 4.75, 84, 960, 0.5)
message(NEXT + 5.8, 84, 1532, 0.5)
swoosh(NEXT + 6.8, 0.5, 2000, 500, 960, 960, 0.5)

# 1:22 The close: the team in a row, a note for each, then together into the mark.
UI_LEVEL = 1.0
END = 82.6
ROW = [421 + i * 98 for i in range(12)]
for i, note in enumerate((62, 65, 69, 70, 72, 74, 77, 81, 82, 84, 86, 89)):
    put('arp', pluck(note, 0.6, 0.5, 1.4), END + 0.25 + i * 0.07, 0.15, pan=pan_x(ROW[i]), verb=0.35, echo=0.3)
LAND = END + 1.75 + 5.5 * 0.025 + 0.75
put('fx', riser(LAND - (END + 1.2), 250, 8000, tone=48), END + 1.2, 0.09, verb=0.25)
reverse_crash(LAND, 1.4, 1.0)
swoosh(END + 1.75, LAND - (END + 1.75), 500, 3000, 960, 960, 1.4, peak=0.92)
for i in range(12):
    # A roll that speeds up into the hit.
    put('drums', pick(SNARES), LAND - (1 - i / 12) ** 1.5, 0.25 * (0.3 + 0.7 * i / 11), verb=0.2)
impact(LAND, 1.4, 1.1)
crash(LAND, 1.0)
for i, note in enumerate((77, 81, 84, 88)):
    put('bell', bell(note, 2.4, 4.6), LAND + i * 0.025, 0.11, verb=0.6, echo=0.2)
chime(END + 3.35, [84, 91], 960, 0.6)

# ---------------------------------------------------------------- mixing


def ducking(release=0.13):
    """How hard each kick pushes the pads and the bass down, per sample."""
    curve = np.zeros(N)
    shape = env(S(0.45), 0.004, release)
    for at, level in KICKS:
        a = S(at)
        b = min(N, a + len(shape))
        if a < N:
            curve[a:b] = np.maximum(curve[a:b], shape[: b - a] * min(level, 1))
    return curve


def impulse(rt=2.4, length=3.2):
    """A synthetic hall: decaying noise in four bands, the high ones dying first."""
    n = S(length)
    t = times(n)
    ir = np.zeros((n, 2))
    for ch in range(2):
        for lo, hi, scale in ((30, 400, 1.15), (400, 2000, 1.0), (2000, 6000, 0.7), (6000, 15000, 0.4)):
            ir[:, ch] += filt(rng.standard_normal(n), 'bandpass', [lo, hi], 2) * np.exp(-6.91 * t / (rt * scale))
    ir *= np.clip(t / 0.006, 0, 1)[:, None]
    ir = np.concatenate([np.zeros((S(0.016), 2)), ir])[:n]
    return ir / np.sqrt(np.sum(ir**2, axis=0, keepdims=True))


def ping_pong(x, delay=0.375, feedback=0.42, taps=6):
    mono = x.mean(axis=1)
    out = np.zeros_like(x)
    for k in range(1, taps + 1):
        d = S(delay * k)
        out[d:, (k - 1) % 2] += mono[:-d] * feedback ** (k - 1)
    return filt(filt(out, 'highpass', 300), 'lowpass', 5000)


K_WEIGHT = (
    ([1.53512485958697, -2.69169618940638, 1.19839281085285], [1.0, -1.69065929318241, 0.73248077421585]),
    ([1.0, -2.0, 1.0], [1.0, -1.99004745483398, 0.99007225036621]),
)


def lufs(x):
    """Integrated loudness, as BS.1770 measures it."""
    y = x
    for b, a in K_WEIGHT:
        y = signal.lfilter(b, a, y, axis=0)
    block, hop = S(0.4), S(0.1)
    power = np.concatenate([np.zeros((1, 2)), np.cumsum(y**2, axis=0)])
    starts = np.arange(0, len(y) - block, hop)
    z = (power[starts + block] - power[starts]) / block
    level = -0.691 + 10 * np.log10(z.sum(axis=1) + 1e-12)
    gate = level > -70
    relative = -0.691 + 10 * np.log10(z[gate].sum(axis=1).mean()) - 10
    gate &= level > relative
    return -0.691 + 10 * np.log10(z[gate].sum(axis=1).mean())


def soft_clip(x, ceiling, knee=0.7):
    u = np.abs(x) / ceiling
    over = u > knee
    u[over] = knee + (1 - knee) * np.tanh((u[over] - knee) / (1 - knee))
    return np.sign(x) * u * ceiling


def true_peak(x):
    return 20 * np.log10(np.max(np.abs(signal.resample_poly(x, 4, 1, axis=0))) + 1e-12)


def db(x):
    return 20 * np.log10(np.sqrt(np.mean(x**2)) + 1e-12)


duck = ducking()
STEMS['pad'] *= (1 - 0.35 * duck)[:, None]
STEMS['bass'] *= (1 - 0.55 * duck)[:, None]
STEMS['arp'] *= (1 - 0.2 * duck)[:, None]
STEMS['pad'] = filt(STEMS['pad'], 'highpass', 140)
STEMS['arp'] = filt(STEMS['arp'], 'highpass', 220)
STEMS['bass'] = filt(STEMS['bass'], 'lowpass', 1400)
STEMS['ui'] = filt(STEMS['ui'], 'highpass', 120)

echo = ping_pong(ECHO)
VERB += echo * 0.3
ir = impulse()
wet = np.stack([signal.oaconvolve(VERB[:, ch], ir[:, ch])[:N] for ch in range(2)], axis=1)
wet = filt(filt(wet, 'highpass', 200), 'lowpass', 9000) * 0.5

mix = sum(STEMS.values()) + echo * 0.6 + wet * (1 - 0.15 * duck)[:, None]
mix = filt(mix, 'highpass', 25)

# The ends: in from silence, and out with the picture.
t = times(N)
mix *= np.clip(t / 0.05, 0, 1)[:, None]
mix *= (0.5 + 0.5 * np.cos(np.pi * np.clip((t - 88.4) / 1.55, 0, 1)))[:, None]

# Loudness: about -15 LUFS, peaks held under -1 dBTP.
target = -15.0
ceiling = 10 ** (-1.6 / 20)
gain = 10 ** ((target - lufs(mix)) / 20)
for _ in range(3):
    out = soft_clip(mix * gain, ceiling)
    gain *= 10 ** ((target - lufs(out)) / 20)
out = soft_clip(mix * gain, ceiling)

sections = [('opening', 0, 12), ('plan to graph', 12, 34), ('team', 34, 50), ('montage and runs', 50, 67), ('agents', 67, 75), ('next', 76, 82), ('close', 82, 90)]
print('stem levels, dB RMS after the master gain:')
for name, stem in list(STEMS.items()) + [('reverb', wet), ('echo', echo * 0.6)]:
    print(f'  {name:7}', '  '.join(f'{label} {db(stem[S(a):S(b)] * gain):6.1f}' for label, a, b in sections))
print('mix by section, dB RMS:', '  '.join(f'{label} {db(out[S(a):S(b)]):6.1f}' for label, a, b in sections))
print(f'loudness {lufs(out):.1f} LUFS, true peak {true_peak(out):.1f} dBTP, master gain {20 * np.log10(gain):.1f} dB')

wavfile.write(sys.argv[1], SR, out.astype(np.float32))
if len(sys.argv) > 2:
    # For checking the mix: every stem, after the master gain.
    np.savez(sys.argv[2], **{name: (stem * gain).astype(np.float32) for name, stem in STEMS.items()}, reverb=(wet * gain).astype(np.float32), echo=(echo * 0.6 * gain).astype(np.float32))
print('wrote', sys.argv[1])
