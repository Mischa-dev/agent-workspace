import type { CSSProperties } from 'react'

/*
 * Motion and type for the trailer, read from the design system's tokens at
 * start-up (tokens.css), so the trailer moves and reads like the app: the
 * same two curves, the same durations, the same type scale. Scene-sized moves
 * are the token curves over a beat or two of the soundtrack.
 */

export type Curve = (x: number) => number

function bezier(x1: number, y1: number, x2: number, y2: number): Curve {
  const cx = 3 * x1
  const bx = 3 * (x2 - x1) - cx
  const ax = 1 - cx - bx
  const cy = 3 * y1
  const by = 3 * (y2 - y1) - cy
  const ay = 1 - cy - by
  const sampleX = (t: number) => ((ax * t + bx) * t + cx) * t
  const sampleY = (t: number) => ((ay * t + by) * t + cy) * t
  const slopeX = (t: number) => (3 * ax * t + 2 * bx) * t + cx
  return (x: number) => {
    if (x <= 0) return 0
    if (x >= 1) return 1
    let t = x
    for (let i = 0; i < 8; i += 1) {
      const error = sampleX(t) - x
      if (Math.abs(error) < 1e-6) return sampleY(t)
      const slope = slopeX(t)
      if (Math.abs(slope) < 1e-6) break
      t -= error / slope
    }
    let lo = 0
    let hi = 1
    t = x
    for (let i = 0; i < 40; i += 1) {
      const value = sampleX(t)
      if (Math.abs(value - x) < 1e-6) break
      if (x > value) lo = t
      else hi = t
      t = (lo + hi) / 2
    }
    return sampleY(t)
  }
}

function tokenValue(name: string) {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
  if (!value) throw new Error(`Missing design token ${name}`)
  return value
}

function curveToken(name: string): Curve {
  const match = /cubic-bezier\(([^)]+)\)/.exec(tokenValue(name))
  if (!match) throw new Error(`${name} is not a cubic-bezier`)
  const [a, b, c, d] = match[1].split(',').map(Number)
  return bezier(a, b, c, d)
}

function secondsToken(name: string) {
  const value = tokenValue(name)
  return value.endsWith('ms') ? parseFloat(value) / 1000 : parseFloat(value)
}

export const ease: { out: Curve; inOut: Curve; linear: Curve } = {
  out: (x) => x,
  inOut: (x) => x,
  linear: (x) => x,
}

/** The token durations, in seconds. */
export const duration = { fast: 0.12, standard: 0.18, slow: 0.26, fade: 1.2 }

/** The soundtrack runs at 120 beats a minute; scene moves land on its beats. */
export const BEAT = 0.5

type TypeFacts = { size: number; line: number; weight: number; family: string }
const typeScale = new Map<string, TypeFacts>()
const TYPE_TOKENS = [
  '--t-display',
  '--t-headline',
  '--t-title',
  '--t-heading',
  '--t-body',
  '--t-ui',
  '--t-ui-strong',
  '--t-label',
  '--t-small',
  '--t-caption',
  '--t-caption-strong',
  '--t-tag',
  '--t-code',
  '--t-code-sm',
] as const
export type TypeToken = (typeof TYPE_TOKENS)[number]

export function initMotion() {
  ease.out = curveToken('--ease-out')
  ease.inOut = curveToken('--ease-in-out')
  duration.fast = secondsToken('--duration-fast')
  duration.standard = secondsToken('--duration-standard')
  duration.slow = secondsToken('--duration-slow')
  duration.fade = secondsToken('--duration-fade')
  const probe = document.createElement('span')
  document.body.append(probe)
  for (const name of TYPE_TOKENS) {
    probe.style.font = `var(${name})`
    const style = getComputedStyle(probe)
    typeScale.set(name, {
      size: parseFloat(style.fontSize),
      line: parseFloat(style.lineHeight),
      weight: Number(style.fontWeight),
      family: style.fontFamily,
    })
  }
  probe.remove()
}

/** A type token at trailer size: the token's weight, family and proportions, scaled up. */
export function type(name: TypeToken, scale = 1, extra: CSSProperties = {}): CSSProperties {
  const facts = typeScale.get(name)
  if (!facts) throw new Error(`Unknown type token ${name}`)
  return {
    fontFamily: facts.family,
    fontWeight: facts.weight,
    fontSize: `${facts.size * scale}px`,
    lineHeight: `${facts.line * scale}px`,
    letterSpacing: scale > 1.5 ? 'var(--tracking-tight)' : undefined,
    ...extra,
  }
}

export const clamp = (x: number, lo = 0, hi = 1) => Math.min(hi, Math.max(lo, x))
export const mix = (a: number, b: number, k: number) => a + (b - a) * k

/**
 * Where along a curve it reached y: the curves here only rise, so this undoes
 * one, and a value eased one way can be eased another.
 */
export function unease(y: number, curve: Curve = ease.out) {
  if (y <= 0) return 0
  if (y >= 1) return 1
  let lo = 0
  let hi = 1
  for (let i = 0; i < 24; i += 1) {
    const mid = (lo + hi) / 2
    if (curve(mid) < y) lo = mid
    else hi = mid
  }
  return (lo + hi) / 2
}

/**
 * How far a space has opened for something arriving at k (on the token
 * ease-out): on the in-out curve instead, since opening it pushes whatever is
 * beside it, and a push that starts at full speed reads as a jump.
 */
export const opening = (k: number) => ease.inOut(unease(k))

/** Progress through [start, start + length], through a curve. */
export function progress(t: number, start: number, length: number, curve: Curve = ease.out) {
  if (length <= 0) return t >= start ? 1 : 0
  return curve(clamp((t - start) / length))
}

/** Enter: rise into place and fade in. */
export function rise(
  t: number,
  start: number,
  { length = 0.6, y = 28, x = 0, from = 1, blur = 0 }: { length?: number; y?: number; x?: number; from?: number; blur?: number } = {},
): CSSProperties {
  const k = progress(t, start, length)
  return {
    opacity: k,
    transform: `translate3d(${x * (1 - k)}px, ${y * (1 - k)}px, 0) scale(${mix(from, 1, k)})`,
    filter: blur ? `blur(${blur * (1 - k)}px)` : undefined,
  }
}

/** Leave: fade out and drift away. */
export function leave(
  t: number,
  start: number,
  { length = 0.5, y = -20, x = 0, to = 1 }: { length?: number; y?: number; x?: number; to?: number } = {},
): CSSProperties {
  const k = progress(t, start, length, ease.inOut)
  return {
    opacity: 1 - k,
    transform: `translate3d(${x * k}px, ${y * k}px, 0) scale(${mix(1, to, k)})`,
  }
}

/** Combines an entrance and an exit on one element. */
export function life(
  t: number,
  start: number,
  end: number,
  enter: Parameters<typeof rise>[2] = {},
  exit: Parameters<typeof leave>[2] = {},
): CSSProperties {
  if (t < end) return rise(t, start, enter)
  return leave(t, end, exit)
}

/** Text typed out at a steady rate. */
export function typed(text: string, t: number, start: number, perSecond = 32) {
  const count = Math.floor(clamp((t - start) * perSecond, 0, text.length))
  return text.slice(0, count)
}

/** A number counting up to its value. */
export function countUp(value: number, t: number, start: number, length = 1.2) {
  return Math.round(value * progress(t, start, length))
}

/** Seeds whose swatch (swatchFor) is the one asked for, so each person and agent gets a chosen color. */
export function seedFor(name: string, swatch: string, swatchFor: (seed: string) => string) {
  for (let i = 0; i < 500; i += 1) {
    const seed = `${name}-${i}`
    if (swatchFor(seed) === swatch) return seed
  }
  return name
}
