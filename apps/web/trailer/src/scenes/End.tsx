import type { CSSProperties } from 'react'

import { Avatar } from '../../../src/ui'
import { agents, people, type Actor } from '../data'
import { clamp, ease, mix, progress, type } from '../motion'
import { Mark } from '../parts/Logo'
import { Line } from '../parts/Text'

/*
 * 1:23. The close. The whole team in a row, people round and agents square as
 * the design system draws them, then they come together: the people into the
 * mark's circle, the agents into its square. The name and the line under it.
 */

const ROW: Actor[] = [
  agents.claude,
  people.priya,
  agents.codex,
  agents.chatgpt,
  people.leo,
  agents.grok,
  agents.muse,
  people.sam,
  agents.hermes,
  agents.openclaw,
  people.maya,
  agents.scout,
]

const SIZE = 76
const GAP = 22
const ROW_Y = 420
const ROW_X = (1920 - (ROW.length * SIZE + (ROW.length - 1) * GAP)) / 2

// The mark, and where its two shapes sit on screen.
const MARK = { size: 290, x: 960 - 145, y: 380 - 145 }
const unit = MARK.size / 512
const CIRCLE = { x: MARK.x + 193 * unit, y: MARK.y + 193 * unit, size: 174 * unit }
const SQUARE = { x: MARK.x + 319 * unit, y: MARK.y + 319 * unit, size: 174 * unit }

const STAGGER = 0.025
const GATHER = 0.75
const T = { gather: 1.75, land: 0, name: 2.75, line: 3.35, out: 6.85 }
// Everyone has arrived once the last pair from the ends of the row lands.
T.land = T.gather + ((ROW.length - 1) / 2) * STAGGER + GATHER

function Member({ actor, index, t }: { actor: Actor; index: number; t: number }) {
  const k = progress(t, 0.25 + index * 0.07, 0.55)
  const middle = (ROW.length - 1) / 2
  const go = progress(t, T.gather + Math.abs(index - middle) * STAGGER, GATHER, ease.inOut)
  const target = actor.kind === 'person' ? CIRCLE : SQUARE
  const x = mix(ROW_X + index * (SIZE + GAP) + SIZE / 2, target.x, go)
  const y = mix(ROW_Y, target.y, go) + (1 - k) * 30
  // Sized rather than scaled, so the faded avatars never share one composited layer (which showed as a box).
  const size = mix(SIZE, target.size, go)
  const fade = progress(t, T.land, 0.4)
  const style: CSSProperties = {
    position: 'absolute',
    display: 'flex',
    left: x - size / 2,
    top: y - size / 2,
    opacity: clamp(k * 1.4) * (1 - fade),
  }
  return (
    <div style={style}>
      <Avatar kind={actor.kind === 'agent' ? 'agent' : 'person'} name={actor.name} seed={actor.id} size={size} />
    </div>
  )
}

export function End({ t }: { t: number }) {
  const glow = progress(t, T.land - 0.1, 1.2)
  const out = progress(t, T.out, 0.5, ease.inOut)
  const push = mix(1, 1.03, progress(t, T.land, 4.5, ease.linear))
  const line = progress(t, T.line, 0.8)
  return (
    <div className="tr-layer" style={{ opacity: 1 - out, transform: `scale(${push})`, transformOrigin: '960px 500px' }}>
      <div
        style={{
          position: 'absolute',
          left: 960 - 360,
          top: 380 - 360,
          width: 720,
          height: 720,
          borderRadius: 'var(--radius-full)',
          background: 'radial-gradient(closest-side, color-mix(in oklch, var(--brand) 24%, transparent), transparent)',
          opacity: glow,
          // Its own layer: drawn in a layer shared with the fading avatars, it showed as a bright box around each.
          willChange: 'transform',
        }}
      />
      {/* The mark waits under the avatars until they have all landed on its shapes, then shows through as they fade. */}
      <div style={{ position: 'absolute', left: MARK.x, top: MARK.y, display: 'flex', visibility: t >= T.land ? 'visible' : 'hidden' }}>
        <Mark at={0} size={MARK.size} t={10} />
      </div>
      {ROW.map((actor, index) => (
        <Member actor={actor} index={index} key={actor.id} t={t} />
      ))}
      <h1 className="tr-title" style={{ ...type('--t-display', 3.7), position: 'absolute', left: 0, right: 0, top: 580, textAlign: 'center' }}>
        <Line at={T.name} stagger={0.1} t={t} text="Agent Workspace" />
      </h1>
      <p
        className="tr-sub"
        style={{
          ...type('--t-title', 1.75),
          position: 'absolute',
          left: 0,
          right: 0,
          top: 722,
          fontWeight: 500,
          textAlign: 'center',
          color: 'var(--sidebar-fg-muted)',
          opacity: line,
          transform: `translate3d(0, ${(1 - line) * 16}px, 0)`,
        }}
      >
        Where your team and your agents build projects together.
      </p>
    </div>
  )
}
