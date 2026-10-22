import type { CSSProperties } from 'react'

import { Avatar, Icon } from '../../../src/ui'
import { agents, type Actor } from '../data'
import { clamp, ease, mix, progress } from '../motion'
import { CodeCard, codeLength, type CodeLine } from '../parts/Code'
import { Swap } from '../parts/Swap'
import { Caption } from '../parts/Text'

/*
 * 1:07. Any agent, from any maker, joins the same way: a roster of the
 * project's agents as the app draws them, and the runner SDK bringing your
 * own one online, typed out in full.
 */

type Presence = { state: 'working' | 'waiting' | 'idle'; label: string; was?: Presence; k?: number }

type Member = { actor: Actor; text: string; meta: string; presence: (t: number) => Presence }

const working = (label: string): Presence => ({ state: 'working', label })
const idle: Presence = { state: 'idle', label: 'Idle' }
/** A presence that changes at `at`, cross-fading from what it was. */
const becomes = (t: number, at: number, from: Presence, to: Presence): Presence => (t < at ? from : { ...to, was: from, k: progress(t, at, 0.35) })

const ROSTER: Member[] = [
  { actor: agents.claude, text: 'Fixes bugs and reviews pull requests.', meta: 'Claude Code · Northwind cloud', presence: () => working('Working on NW-16') },
  { actor: agents.codex, text: 'Takes tasks from Ready to a pull request.', meta: 'Codex · Northwind cloud', presence: () => working('Working on NW-26') },
  {
    actor: agents.chatgpt,
    text: 'Turns plans and notes into tasks.',
    meta: 'ChatGPT agent · connected over MCP',
    presence: (t) => becomes(t, 3.1, idle, working('Working on NW-19')),
  },
  { actor: agents.grok, text: 'Writes the docs and the changelog.', meta: 'Grok · connected over MCP', presence: () => working('Working on NW-28') },
  { actor: agents.muse, text: 'Designs screens and draws on whiteboards.', meta: 'Meta Muse · connected over MCP', presence: () => working('Working on NW-27') },
  { actor: agents.hermes, text: 'Load tests, and watches for slow pages.', meta: 'Hermes · runner SDK', presence: () => ({ state: 'waiting', label: 'Waiting on you' }) },
  { actor: agents.openclaw, text: 'Runs the backups and the nightly checks.', meta: 'OpenClaw · runner SDK', presence: () => idle },
  {
    actor: agents.scout,
    text: 'Our own harness, running on Leo’s laptop.',
    meta: 'Your own harness · runner SDK',
    presence: (t) => becomes(t, T.scout + 1.1, idle, working('Working on NW-17')),
  },
]

const K = 'tr-tok-key'
const F = 'tr-tok-fn'
const S = 'tr-tok-str'
const D = 'tr-tok-dim'
const OK = 'tr-tok-ok'

const CODE: CodeLine[] = [
  [[K, 'import'], ['', ' { Runner, runnerSettingsFromEnv } '], [K, 'from'], ['', ' '], [S, "'@agent-workspace/runner'"]],
  [],
  [[K, 'const'], ['', ' runner = '], [K, 'new'], ['', ' '], [F, 'Runner'], ['', '({']],
  [['', '  ...'], [F, 'runnerSettingsFromEnv'], ['', '(),']],
  [['', '  '], [K, 'async'], ['', ' '], [F, 'onRequest'], ['', '(request) {']],
  [['', '    '], [K, 'await'], ['', ' request.'], [F, 'progress'], ['', '('], [S, "'Reading the repository'"], ['', ')']],
  [['', '    '], [K, 'const'], ['', ' answer = '], [K, 'await'], ['', ' scout.'], [F, 'run'], ['', '(request.prompt)']],
  [['', '    '], [K, 'await'], ['', ' request.'], [F, 'reply'], ['', '(answer)']],
  [['', '  },']],
  [['', '})']],
  [],
  [['', 'runner.'], [F, 'listen'], ['', '(8787)']],
]

const T = {
  code: 1.0,
  typing: 1.45,
  rate: 150,
  run: 0,
  online: 0,
  scout: 0,
  exit: 8.55,
}
T.run = T.typing + codeLength(CODE) / T.rate + 0.25
T.online = T.run + 0.5
T.scout = T.online + 0.15

const CARD = { w: 440, gap: 24, x: 900, y: 204 }
const ARRIVE = 0.45
const STAGGER = 0.09

function ProfileCard({ index, member, t }: { index: number; member: Member; t: number }) {
  const start = member.actor === agents.scout ? T.scout : ARRIVE + index * STAGGER
  const k = progress(t, start, 0.55)
  const out = progress(t, T.exit + index * 0.03, 0.4, ease.inOut)
  const presence = member.presence(t)
  const fresh = member.actor === agents.scout ? 1 - progress(t, T.scout + 0.5, 1.2) : 0
  const style: CSSProperties = {
    opacity: k * (1 - out),
    transform: `translate3d(0, ${(1 - k) * 26 - out * 24}px, 0) scale(${mix(0.96, 1, k)})`,
    boxShadow: fresh > 0 && k > 0 ? `var(--shadow-popover), 0 0 0 ${2 * fresh + 1}px color-mix(in oklch, var(--accent) ${Math.round(fresh * 100)}%, transparent)` : undefined,
  }
  return (
    <li className="aw-profilecard tr-profilecard" style={style}>
      <div className="aw-profilecard__head">
        <Avatar kind="agent" name={member.actor.name} seed={member.actor.id} size={36} />
        <div className="aw-profilecard__name">
          <strong>{member.actor.name}</strong>
          <span className="aw-profilecard__meta">@{member.actor.handle}</span>
        </div>
      </div>
      <p className="aw-profilecard__text">{member.text}</p>
      <p className="aw-profilecard__meta">{member.meta}</p>
      <p className="aw-profilecard__presence">
        {presence.was ? (
          <Swap from={<PresenceLine presence={presence.was} />} id={`presence-${member.actor.id}`} inline k={presence.k ?? 1} to={<PresenceLine presence={presence} />} />
        ) : (
          <PresenceLine presence={presence} />
        )}
      </p>
    </li>
  )
}

function PresenceLine({ presence }: { presence: Presence }) {
  return (
    <span className="tr-presence">
      <span aria-hidden className={`aw-agentdot aw-agentdot--${presence.state}`} />
      {presence.label}
    </span>
  )
}

export function Agents({ t }: { t: number }) {
  const codeIn = progress(t, T.code, 0.6)
  const codeOut = progress(t, T.exit - 0.1, 0.45, ease.inOut)
  const shown = Math.floor(clamp((t - T.typing) * T.rate, 0, codeLength(CODE)))
  // The terminal under the editor keeps its two lines from the start, so the card never grows.
  const terminal: CodeLine[] = [
    [[D, '$ '], ['', 'node scout.ts'.slice(0, Math.floor(clamp((t - T.run) * 40, 0, 13)))]],
    [[OK, '✓ Scout is listening for Northwind']],
  ]
  const push = mix(1, 1.02, progress(t, 0.4, 8.2, ease.linear))
  return (
    <div className="tr-layer">
      <Caption
        at={0.15}
        exitAt={T.exit - 0.15}
        kicker="Any agent"
        kickerIcon={<Icon name="bot" size={26} />}
        sub="Connect agents over MCP, or build your own with the runner SDK. Each one gets a name, a tag and its own work."
        t={t}
        title={['Bring any agent.', 'Or build *your own.*']}
        titleScale={2.3}
        width={740}
        x={110}
        y={96}
      />
      <CodeCard
        file="scout.ts"
        lines={CODE}
        shown={shown}
        style={{
          left: 110,
          top: 486,
          width: 740,
          opacity: codeIn * (1 - codeOut),
          transform: `translate3d(0, ${(1 - codeIn) * 30 - codeOut * 24}px, 0)`,
        }}
        terminal={terminal}
        terminalShown={[1, progress(t, T.online, 0.25)]}
        typing={t >= T.typing && shown < codeLength(CODE)}
      />
      <div className="tr-layer" style={{ transform: `scale(${push})`, transformOrigin: '1350px 540px' }}>
        <ul className="tr-roster" style={{ left: CARD.x, top: CARD.y, gridTemplateColumns: `repeat(2, ${CARD.w}px)`, gap: CARD.gap }}>
          {ROSTER.map((member, index) => (
            <ProfileCard index={index} key={member.actor.id} member={member} t={t} />
          ))}
        </ul>
      </div>
    </div>
  )
}
