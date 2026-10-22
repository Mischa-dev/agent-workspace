import rough from 'roughjs'
import type { CSSProperties, ReactNode } from 'react'

import { clamp, progress, type } from '../motion'

/*
 * Whiteboard sketches, drawn the way the app's whiteboards draw them: rough.js
 * strokes and the Excalifont hand. Each shape draws itself in over a beat,
 * outline first and then its fill, like a pen going round it.
 */

const generator = rough.generator()

type Point = [number, number]

export type Shape =
  | { kind: 'rect'; x: number; y: number; w: number; h: number; fill?: string; seed: number }
  | { kind: 'diamond'; x: number; y: number; w: number; h: number; fill?: string; seed: number }
  | { kind: 'arrow'; points: Point[]; seed: number }

type Stroke = { d: string; fill?: string; stroke?: string; width: number }

const INK = 'var(--content-fg)'

function strokes(shape: Shape): Stroke[] {
  const options = {
    seed: shape.seed,
    roughness: 1.1,
    bowing: 1,
    stroke: INK,
    strokeWidth: 2,
    fill: 'fill' in shape ? shape.fill : undefined,
    fillStyle: 'solid',
  }
  const drawables =
    shape.kind === 'rect'
      ? [generator.rectangle(shape.x, shape.y, shape.w, shape.h, options)]
      : shape.kind === 'diamond'
        ? [
            generator.polygon(
              [
                [shape.x + shape.w / 2, shape.y],
                [shape.x + shape.w, shape.y + shape.h / 2],
                [shape.x + shape.w / 2, shape.y + shape.h],
                [shape.x, shape.y + shape.h / 2],
              ],
              options,
            ),
          ]
        : arrow(shape.points, options)
  return drawables.flatMap((drawable) =>
    generator.toPaths(drawable).map((path) => ({
      d: path.d,
      fill: path.fill && path.fill !== 'none' ? path.fill : undefined,
      stroke: path.stroke && path.stroke !== 'none' ? path.stroke : undefined,
      width: path.strokeWidth,
    })),
  )
}

function arrow(points: Point[], options: Record<string, unknown>) {
  const line = points.length > 2 ? generator.curve(points, options) : generator.line(points[0][0], points[0][1], points[1][0], points[1][1], options)
  const [x1, y1] = points[points.length - 2]
  const [x2, y2] = points[points.length - 1]
  const angle = Math.atan2(y2 - y1, x2 - x1)
  const head = (turn: number) => {
    const a = angle + Math.PI + turn
    return generator.line(x2, y2, x2 + Math.cos(a) * 18, y2 + Math.sin(a) * 18, options)
  }
  return [line, head(0.45), head(-0.45)]
}

const cache = new Map<string, Stroke[]>()
function cached(shape: Shape) {
  const key = JSON.stringify(shape)
  let found = cache.get(key)
  if (!found) {
    found = strokes(shape)
    cache.set(key, found)
  }
  return found
}

/** One shape, drawn in from `at` over `length` seconds. */
export function Drawn({ at, length = 0.6, shape, t }: { at: number; length?: number; shape: Shape; t: number }) {
  const k = progress(t, at, length)
  if (k <= 0) return null
  const paths = cached(shape)
  const outline = paths.filter((path) => path.stroke)
  const fills = paths.filter((path) => path.fill)
  const fillIn = clamp((k - 0.55) / 0.45)
  return (
    <g>
      {fills.map((path, index) => (
        <path d={path.d} key={`f${index}`} style={{ fill: path.fill, opacity: fillIn }} />
      ))}
      {outline.map((path, index) => (
        <path
          d={path.d}
          key={`s${index}`}
          pathLength={1}
          style={{
            fill: 'none',
            stroke: path.stroke,
            strokeWidth: path.width,
            strokeLinecap: 'round',
            strokeDasharray: '1 1',
            strokeDashoffset: 1 - clamp(k * 1.25),
          }}
        />
      ))}
    </g>
  )
}

/** Handwriting, which appears a few letters at a time. */
export function Hand({
  at,
  children,
  size = 26,
  style,
  t,
  x,
  y,
  anchor = 'middle',
}: {
  at: number
  children: string
  size?: number
  style?: CSSProperties
  t: number
  x: number
  y: number
  anchor?: 'start' | 'middle'
}) {
  const shown = Math.floor(clamp((t - at) * 40, 0, children.length))
  if (shown <= 0) return null
  return (
    <text
      dominantBaseline="central"
      style={{ fontFamily: 'Excalifont', fontSize: size, fill: INK, ...style }}
      textAnchor={anchor}
      x={x}
      y={y}
    >
      {children.slice(0, shown)}
    </text>
  )
}

/** Someone else's pointer on the board, with their name, as the whiteboard shows collaborators. */
export function Collaborator({ color, name, x, y, opacity = 1 }: { color: string; name: ReactNode; x: number; y: number; opacity?: number }) {
  return (
    <div style={{ position: 'absolute', left: x, top: y, opacity, pointerEvents: 'none' }}>
      <svg height="26" style={{ position: 'absolute', left: -2, top: -2, overflow: 'visible' }} viewBox="0 0 18 26" width="18">
        <path d="M1 1 L1 20.5 L5.9 16.2 L9.4 24.2 L12.6 22.8 L9.2 15 L15.8 15 Z" style={{ fill: color, stroke: 'var(--on-swatch)', strokeWidth: 1.5, strokeLinejoin: 'round' }} />
      </svg>
      <span
        style={{
          position: 'absolute',
          left: 16,
          top: 20,
          padding: 'var(--space-half) var(--space-2)',
          borderRadius: 'var(--radius-sm)',
          background: color,
          color: 'var(--on-swatch)',
          whiteSpace: 'nowrap',
          ...type('--t-caption-strong'),
        }}
      >
        {name}
      </span>
    </div>
  )
}
