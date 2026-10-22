import type { CSSProperties, ReactNode } from 'react'

import { clamp, ease, leave, progress, type, type TypeToken } from '../motion'

/*
 * Kinetic type. Each line is a mask; its words rise into it one after another,
 * on the design system's ease-out curve. Words wrapped in *stars* take the
 * accent color, one word or a run of them.
 */

export function Line({
  at,
  className,
  length = 0.7,
  stagger = 0.06,
  style,
  t,
  text,
}: {
  at: number
  className?: string
  length?: number
  stagger?: number
  style?: CSSProperties
  t: number
  text: string
}) {
  const words = text.split(' ')
  // A run of words between stars takes the accent: "*your own.*".
  let open = false
  const accents = words.map((raw) => {
    const starts = raw.startsWith('*')
    const accent = open || starts
    open = (open || starts) && !(raw.endsWith('*') && (raw.length > 1 || !starts))
    return accent
  })
  return (
    <span className={['tr-line', className].filter(Boolean).join(' ')} style={style}>
      {words.map((raw, index) => {
        const accent = accents[index]
        const word = raw.replace(/\*/g, '')
        const k = progress(t, at + index * stagger, length)
        return (
          <span
            className={accent ? 'tr-word tr-accent' : 'tr-word'}
            key={`${index}-${word}`}
            style={{ transform: `translate3d(0, ${(1 - k) * 110}%, 0)`, opacity: clamp(k * 1.6) }}
          >
            {word}
            {index < words.length - 1 ? ' ' : ''}
          </span>
        )
      })}
    </span>
  )
}

/** How long a line takes to finish arriving. */
export function lineLength(text: string, length = 0.7, stagger = 0.06) {
  return length + (text.split(' ').length - 1) * stagger
}

export function Caption({
  align = 'left',
  at,
  exitAt,
  exitTo = { y: -24 },
  kicker,
  kickerIcon,
  sub,
  subAt,
  style,
  t,
  title,
  titleScale = 3.1,
  titleToken = '--t-display',
  width = 640,
  x,
  y,
}: {
  align?: 'left' | 'center'
  at: number
  exitAt?: number
  /** Which way it drifts as it leaves. */
  exitTo?: { x?: number; y?: number }
  kicker?: string
  kickerIcon?: ReactNode
  sub?: string | string[]
  subAt?: number
  style?: CSSProperties
  t: number
  title: string[]
  titleScale?: number
  titleToken?: TypeToken
  width?: number
  x: number
  y: number
}) {
  const exit = exitAt !== undefined && t >= exitAt ? leave(t, exitAt, { x: exitTo.x ?? 0, y: exitTo.y ?? 0, length: 0.45 }) : {}
  const titleStart = at + (kicker ? 0.12 : 0)
  const titleEnd = titleStart + title.reduce((sum, line, index) => Math.max(sum, index * 0.12 + lineLength(line)), 0)
  const subStart = subAt ?? titleEnd - 0.3
  const subs = sub === undefined ? [] : Array.isArray(sub) ? sub : [sub]
  return (
    <div
      className="tr-caption"
      style={{
        left: x,
        top: y,
        width,
        textAlign: align,
        justifyItems: align === 'center' ? 'center' : 'start',
        ...style,
        ...exit,
      }}
    >
      {kicker ? (
        <span
          className="tr-kicker"
          style={{
            ...type('--t-label', 2),
            opacity: progress(t, at, 0.5),
            transform: `translate3d(0, ${(1 - progress(t, at, 0.6)) * 16}px, 0)`,
          }}
        >
          {kickerIcon}
          {kicker}
        </span>
      ) : null}
      <h2 className="tr-title" style={type(titleToken, titleScale)}>
        {title.map((line, index) => (
          <Line at={titleStart + index * 0.12} key={line} t={t} text={line} />
        ))}
      </h2>
      {subs.map((line, index) => {
        const start = subStart + index * 0.9
        const k = progress(t, start, 0.7)
        return (
          <p
            className="tr-sub"
            key={line}
            style={{
              ...type('--t-body', 1.85),
              opacity: k,
              transform: `translate3d(0, ${(1 - k) * 18}px, 0)`,
            }}
          >
            {line}
          </p>
        )
      })}
    </div>
  )
}

/** One line of big type in the middle of the frame, which can be swapped for the next on a beat. */
export function Headline({
  at,
  exitAt,
  scale = 3.6,
  style,
  t,
  text,
  token = '--t-display',
}: {
  at: number
  exitAt?: number
  scale?: number
  style?: CSSProperties
  t: number
  text: string
  token?: TypeToken
}) {
  if (t < at - 0.01) return null
  const out = exitAt !== undefined ? progress(t, exitAt, 0.4, ease.inOut) : 0
  if (out >= 1) return null
  return (
    <div
      className="tr-title"
      style={{
        ...type(token, scale),
        textAlign: 'center',
        opacity: 1 - out,
        transform: `translate3d(0, ${-40 * out}px, 0)`,
        ...style,
      }}
    >
      <Line at={at} t={t} text={text} />
    </div>
  )
}
