import type { CSSProperties } from 'react'

import { Avatar, Icon, type IconName } from '../../../src/ui'
import { agents, type Actor } from '../data'
import { clamp, ease, mix, progress, rise, type } from '../motion'
import { Headline } from '../parts/Text'

/*
 * 0:00. The problem: agents everywhere, work scattered across tabs. The tabs
 * then fall into the middle of the frame, where the mark appears.
 */

type TabKind = 'terminal' | 'chat' | 'list' | 'doc' | 'grid'
// Each agent in its own tab, under its own logo, between the tabs for everything else.
const TABS: { x: number; y: number; w: number; h: number; r: number; title: string; icon: IconName; kind: TabKind; agent?: Actor }[] = [
  { x: 120, y: 110, w: 380, h: 240, r: -4, title: 'Claude Code — terminal', icon: 'terminal', kind: 'terminal', agent: agents.claude },
  { x: 610, y: 64, w: 330, h: 206, r: 3, title: 'ChatGPT', icon: 'message', kind: 'chat', agent: agents.chatgpt },
  { x: 1500, y: 96, w: 350, h: 244, r: 5, title: 'Pull requests', icon: 'pull-request', kind: 'list' },
  { x: 1110, y: 150, w: 310, h: 196, r: -3, title: 'Issues', icon: 'work', kind: 'list' },
  { x: 96, y: 720, w: 350, h: 250, r: 3, title: 'Spec — final v2', icon: 'list', kind: 'doc' },
  { x: 560, y: 790, w: 310, h: 200, r: -5, title: 'Notes', icon: 'pencil', kind: 'doc' },
  { x: 1480, y: 730, w: 370, h: 240, r: -4, title: 'Roadmap.xlsx', icon: 'board', kind: 'grid' },
  { x: 1090, y: 806, w: 320, h: 210, r: 4, title: 'Codex — terminal', icon: 'terminal', kind: 'terminal', agent: agents.codex },
  { x: -20, y: 430, w: 270, h: 176, r: -6, title: 'Grok', icon: 'messages', kind: 'chat', agent: agents.grok },
  { x: 1690, y: 440, w: 250, h: 176, r: 6, title: 'Muse', icon: 'messages', kind: 'chat', agent: agents.muse },
]

function Bars({ widths, style }: { widths: number[]; style?: CSSProperties }) {
  return (
    <div style={{ display: 'grid', gap: 'var(--space-2)', ...style }}>
      {widths.map((width, index) => (
        <span
          key={index}
          style={{ display: 'block', height: 10, width: `${width}%`, borderRadius: 'var(--radius-xs)', background: 'var(--content-border)' }}
        />
      ))}
    </div>
  )
}

function TabBody({ agent, kind }: { agent?: Actor; kind: TabKind }) {
  if (kind === 'terminal') {
    return (
      <div style={{ ...type('--t-code-sm'), padding: 'var(--space-3) var(--space-4)', color: 'var(--rail-fg)', display: 'grid', gap: 2 }}>
        <span>$ agent --task export</span>
        <span style={{ color: 'var(--sidebar-fg-muted)' }}>reading repository…</span>
        <span style={{ color: 'var(--sidebar-fg-muted)' }}>12 files changed</span>
        <span style={{ color: 'var(--presence-online)' }}>✓ tests passed</span>
        <span>waiting for review_</span>
      </div>
    )
  }
  if (kind === 'chat') {
    // The agent's lines on the left, under its logo; yours on the right.
    return (
      <div style={{ display: 'grid', gap: 'var(--space-2)', padding: 'var(--space-3) var(--space-4)' }}>
        {[62, 44, 70].map((width, index) => {
          const theirs = index !== 1
          return (
            <div key={index} style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-2)', justifyContent: theirs ? 'flex-start' : 'flex-end' }}>
              {theirs && agent ? <Avatar kind="agent" name={agent.name} seed={agent.id} size={20} /> : null}
              <span
                style={{
                  width: `${width}%`,
                  height: 26,
                  borderRadius: 'var(--radius-md)',
                  background: theirs ? 'var(--surface-sunken)' : 'var(--accent-soft)',
                }}
              />
            </div>
          )
        })}
      </div>
    )
  }
  if (kind === 'grid') {
    return (
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(4, 1fr)', gap: 1, margin: 'var(--space-3)', background: 'var(--content-border)' }}>
        {Array.from({ length: 20 }, (_, index) => (
          <span key={index} style={{ height: 22, background: index < 4 ? 'var(--surface-sunken)' : 'var(--surface-raised)' }} />
        ))}
      </div>
    )
  }
  if (kind === 'doc') return <Bars style={{ padding: 'var(--space-4)' }} widths={[46, 92, 84, 88, 60]} />
  return (
    <div style={{ display: 'grid', gap: 'var(--space-2-5)', padding: 'var(--space-3) var(--space-4)' }}>
      {[80, 66, 72, 58].map((width, index) => (
        <div key={index} style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-2)' }}>
          <span style={{ width: 12, height: 12, borderRadius: 'var(--radius-full)', border: '2px solid var(--control-border)' }} />
          <Bars style={{ flex: 1 }} widths={[width]} />
        </div>
      ))}
    </div>
  )
}

function Tab({ k, pull, tab }: { k: number; pull: number; tab: (typeof TABS)[number] }) {
  const dark = tab.kind === 'terminal'
  const cx = tab.x + tab.w / 2
  const cy = tab.y + tab.h / 2
  // Pulled toward the middle of the frame, shrinking as they go.
  const dx = (960 - cx) * pull
  const dy = (520 - cy) * pull
  const drift = (1 - pull) * 1
  return (
    <div
      style={{
        position: 'absolute',
        left: tab.x,
        top: tab.y,
        width: tab.w,
        height: tab.h,
        overflow: 'hidden',
        borderRadius: 'var(--radius-lg)',
        background: dark ? 'var(--rail-bg)' : 'var(--surface-raised)',
        boxShadow: 'var(--shadow-dialog)',
        outline: dark ? '1px solid var(--sidebar-border)' : undefined,
        opacity: clamp(k * 1.4) * (1 - clamp((pull - 0.75) * 4)),
        transform: `translate3d(${dx}px, ${dy + (1 - k) * 40}px, 0) rotate(${tab.r * drift}deg) scale(${mix(0.86, 1, k) * mix(1, 0.08, pull)})`,
      }}
    >
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 'var(--space-2)',
          height: 34,
          padding: '0 var(--space-3)',
          borderBottom: `1px solid ${dark ? 'var(--sidebar-border)' : 'var(--content-border)'}`,
          color: dark ? 'var(--sidebar-fg-muted)' : 'var(--content-fg-muted)',
          ...type('--t-caption-strong'),
        }}
      >
        <span style={{ display: 'flex', gap: 5 }}>
          {[0, 1, 2].map((dot) => (
            <span key={dot} style={{ width: 9, height: 9, borderRadius: 'var(--radius-full)', background: dark ? 'var(--sidebar-border)' : 'var(--content-border)' }} />
          ))}
        </span>
        {tab.agent ? <Avatar kind="agent" name={tab.agent.name} seed={tab.agent.id} size={18} /> : <Icon name={tab.icon} size={14} />}
        <span>{tab.title}</span>
      </div>
      <TabBody agent={tab.agent} kind={tab.kind} />
    </div>
  )
}

export function Hook({ t }: { t: number }) {
  // The second line rolls through what agents do.
  const verbs = [
    { at: 1.75, text: 'They write *code.*' },
    { at: 2.55, text: 'They review *pull requests.*' },
    { at: 3.35, text: 'They plan *whole projects.*' },
  ]
  const lift = progress(t, 1.6, 0.6, ease.inOut)
  const pull = progress(t, 5.9, 0.95, ease.inOut)
  return (
    <div className="tr-layer">
      {TABS.map((tab, index) => (
        <Tab k={progress(t, 4.25 + index * 0.075, 0.65)} key={tab.title + index} pull={pull} tab={tab} />
      ))}
      <div className="tr-center" style={{ alignContent: 'center' }}>
        <div style={{ display: 'grid', justifyItems: 'center', gap: 'var(--space-4)' }}>
          <div
            style={{
              transform: `translate3d(0, ${-30 * lift}px, 0)`,
              opacity: mix(1, 0.42, lift) * (1 - progress(t, 4.1, 0.4, ease.inOut)),
            }}
          >
            <Headline at={0.3} t={t} text="Your team has agents now." />
          </div>
          <div style={{ position: 'relative', height: 130, width: 1600 }}>
            {verbs.map((verb, index) => {
              const next = verbs[index + 1]
              const exitAt = next ? next.at - 0.05 : 4.1
              if (t < verb.at - 0.01 || t > exitAt + 0.45) return null
              return (
                <div key={verb.text} style={{ position: 'absolute', inset: 0 }}>
                  <Headline at={verb.at} exitAt={exitAt} t={t} text={verb.text} />
                </div>
              )
            })}
          </div>
        </div>
      </div>
      {t > 4.4 ? (
        <div className="tr-center">
          <div style={{ ...rise(t, 4.55, { y: 30 }), opacity: clamp(progress(t, 4.55, 0.6)) * (1 - progress(t, 5.95, 0.45, ease.inOut)) }}>
            <Headline at={4.55} scale={2.9} t={t} text="And they’re stuck in a dozen tabs." />
          </div>
        </div>
      ) : null}
    </div>
  )
}
