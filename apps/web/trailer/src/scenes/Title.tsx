import type { CSSProperties } from 'react'

import { Avatar } from '../../../src/ui'
import { agents, people, type Actor } from '../data'
import { clamp, ease, mix, progress, type } from '../motion'
import { Mark } from '../parts/Logo'
import { Line } from '../parts/Text'

// Under the line, the team it's talking about: people round and agents square, as the app draws them.
const TEAM: Actor[] = [
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
]
const SIZE = 60
const TEAM_AT = 2.25

/** The name, once the tabs have fallen into the mark. */
export function Title({ t, end }: { t: number; end: number }) {
  const out = progress(t, end - 0.7, 0.7, ease.inOut)
  // A slow push the whole time the card is up, and a glow that breathes behind the mark.
  const push = mix(1, 1.04, progress(t, 0, end, ease.linear))
  const glow = progress(t, 0.15, 1.4) * (0.86 + 0.14 * Math.cos((2 * Math.PI * Math.max(0, t - 1.55)) / 3))
  const swell = 1 + 0.05 * (1 - Math.cos((2 * Math.PI * Math.max(0, t - 1.55)) / 3)) * 0.5
  return (
    <div className="tr-center" style={{ opacity: 1 - out, transform: `scale(${push * mix(1, 0.94, out)})` }}>
      <div style={{ display: 'grid', justifyItems: 'center', gap: 'var(--space-6)', marginTop: -40 }}>
        <div style={{ position: 'relative', width: 230, height: 230 }}>
          <div
            style={{
              position: 'absolute',
              inset: -120,
              borderRadius: 'var(--radius-full)',
              background: 'radial-gradient(closest-side, color-mix(in oklch, var(--brand) 22%, transparent), transparent)',
              opacity: glow,
              transform: `scale(${swell})`,
              willChange: 'transform',
            }}
          />
          <div style={{ position: 'relative' }}>
            <Mark at={0.2} size={230} t={t} />
          </div>
        </div>
        <h1 className="tr-title" style={{ ...type('--t-display', 3.7), textAlign: 'center' }}>
          <Line at={0.95} stagger={0.1} t={t} text="Agent Workspace" />
        </h1>
        <p
          className="tr-sub"
          style={{
            ...type('--t-title', 1.75),
            fontWeight: 500,
            textAlign: 'center',
            color: 'var(--sidebar-fg-muted)',
            opacity: progress(t, 1.6, 0.8),
            transform: `translate3d(0, ${(1 - progress(t, 1.6, 0.8)) * 16}px, 0)`,
          }}
        >
          Where your team and your agents build projects together.
        </p>
        {/* Out of the flow, so the mark stays where the tabs fell. */}
        <div style={{ position: 'relative', justifySelf: 'stretch', height: 0 }}>
          <div style={{ position: 'absolute', top: 'var(--space-6)', left: 0, right: 0, display: 'flex', justifyContent: 'center', gap: 'var(--space-5)' }}>
            {TEAM.map((actor, index) => (
              <Member actor={actor} index={index} key={actor.id} t={t} />
            ))}
          </div>
        </div>
      </div>
    </div>
  )
}

function Member({ actor, index, t }: { actor: Actor; index: number; t: number }) {
  // From the middle out, so the row opens like the line above it.
  const middle = (TEAM.length - 1) / 2
  const k = progress(t, TEAM_AT + Math.abs(index - middle) * 0.07, 0.55, ease.out)
  const style: CSSProperties = {
    display: 'flex',
    opacity: clamp(k * 1.5),
    transform: `translate3d(0, ${(1 - k) * 22}px, 0) scale(${mix(0.8, 1, k)})`,
  }
  return (
    <div style={style}>
      <Avatar kind={actor.kind} name={actor.name} seed={actor.id} size={SIZE} />
    </div>
  )
}
