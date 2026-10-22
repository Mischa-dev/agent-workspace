import { FileText, Laptop, Sparkles } from 'lucide-react'
import type { CSSProperties, ReactNode } from 'react'

import { ActorTag, Avatar, Button, Icon, StatusChip } from '../../../src/ui'
import { agents, builtinAgent as helper, message, people, type Actor } from '../data'
import { ease, mix, progress, type, typed } from '../motion'
import { Arrive, Message, Reactions, reaction, Timeline, WritingRow } from '../parts/Chat'
import { Swap } from '../parts/Swap'
import { Caption } from '../parts/Text'

/*
 * 1:16. What's being built next, each in a tile drawn from the kit: docs and
 * sheets that go to Google in a click, any machine paired as a runner with a
 * short code, and an agent that's ready the day you sign up.
 */

const TILE = { w: 540, h: 452, gap: 32, x: 118, y: 388 }
const ARRIVE = [0.4, 0.55, 0.7]
const EXIT = 6.8

function Tile({ children, desc, head, index, t }: { children: ReactNode; desc: string; head: ReactNode; index: number; t: number }) {
  const k = progress(t, ARRIVE[index], 0.6)
  const out = progress(t, EXIT + index * 0.05, 0.4, ease.inOut)
  const style: CSSProperties = {
    left: TILE.x + index * (TILE.w + TILE.gap),
    top: TILE.y,
    width: TILE.w,
    height: TILE.h,
    opacity: k * (1 - out),
    transform: `translate3d(0, ${(1 - k) * 40 - out * 30}px, 0) scale(${mix(0.96, 1, k)})`,
  }
  return (
    <section className="tr-tile" style={style}>
      <header className="tr-tile__head">
        {head}
        <span className="tr-tile__chip">
          <StatusChip tone="agent">Coming soon</StatusChip>
        </span>
      </header>
      <div className="tr-tile__body">{children}</div>
      <p className="tr-tile__desc">{desc}</p>
    </section>
  )
}

/* Docs and sheets */

const SHEET = [
  ['Area', 'Owner', 'Status'],
  ['Export', 'Codex', 'In review'],
  ['Board', 'Sam Okafor', 'Done'],
  ['Docs', 'Grok', 'In progress'],
  ['Webhooks', 'Hermes', 'Ready'],
]
// Who each row's owner is, so the owner cell carries their avatar.
const OWNERS: Record<string, Actor> = { Codex: agents.codex, 'Sam Okafor': people.sam, Grok: agents.grok, Hermes: agents.hermes }

// Grok updates its own row while the sheet is open: its cursor lands on the cell, and the new status types over the old.
const EDIT = { at: 4.3, row: 3, col: 2, text: 'In review', clear: 0.45, leave: 6.0 }

function Docs({ t }: { t: number }) {
  const start = ARRIVE[0] + 0.7
  const press = 3.35
  const sent = t >= press + 0.12
  const editing = progress(t, EDIT.at, 0.25) * (1 - progress(t, EDIT.leave, 0.4))
  const cell = (r: number, c: number, text: string) => {
    if (r === 0) return text
    const at = start + (r - 1) * 0.3 + c * 0.08
    if (r === EDIT.row && c === EDIT.col && t >= EDIT.at + EDIT.clear) return typed(EDIT.text, t, EDIT.at + EDIT.clear, 16)
    const shown = typed(text, t, at, 40)
    const owner = c === 1 ? OWNERS[text] : undefined
    if (!owner) return shown
    return (
      <span className="tr-sheet__owner">
        <span style={{ display: 'inline-flex', opacity: progress(t, at, 0.25) }}>
          <Avatar kind={owner.kind} name={owner.name} seed={owner.id} size={16} />
        </span>
        {shown}
      </span>
    )
  }
  return (
    <div className="tr-tile__pad tr-doc">
      <h3 className="tr-doc__title">1.4 launch notes</h3>
      <p className="tr-doc__text">What ships on Nov 14, and who owns each part.</p>
      <div className="tr-sheet" role="table">
        {SHEET.map((row, r) => (
          <div className={r === 0 ? 'tr-sheet__row is-head' : 'tr-sheet__row'} key={row[0]} role="row">
            <span className="tr-sheet__num">{r === 0 ? '' : r}</span>
            {row.map((text, c) => {
              const edited = r === EDIT.row && c === EDIT.col && editing > 0
              return (
                <span
                  className={edited ? 'tr-sheet__cell is-editing' : 'tr-sheet__cell'}
                  key={c}
                  role="cell"
                  style={edited ? ({ '--edit': editing } as CSSProperties) : undefined}
                >
                  {cell(r, c, text)}
                  {edited ? (
                    <span className="tr-sheet__flag">
                      <Avatar kind="agent" name={agents.grok.name} seed={agents.grok.id} size={14} />
                      {agents.grok.name}
                    </span>
                  ) : null}
                </span>
              )
            })}
          </div>
        ))}
      </div>
      <div className="tr-doc__actions">
        <Button className={t >= press && t < press + 0.16 ? 'tr-pressed' : undefined} icon="external" size="sm">
          Open in Google Sheets
        </Button>
        {sent ? (
          <span style={{ opacity: progress(t, press + 0.12, 0.3) }}>
            <StatusChip icon="circle-check" tone="success">
              Copied to Google
            </StatusChip>
          </span>
        ) : null}
      </div>
    </div>
  )
}

/* Pair any machine */

const CODE = 'K7Q-2MX'

const PAIRED = 3.55

function Pair({ t }: { t: number }) {
  const start = ARRIVE[1] + 0.75
  const command = 'npx @agent-workspace/runner pair K7Q-2MX'
  const answered = start + command.length / 34 + 0.3
  return (
    <div className="tr-tile__pad tr-pair">
      <p className="tr-doc__text">Pair a runner with this code. It expires in 10 minutes.</p>
      <div className="tr-pair__code" style={type('--t-code', 2.4, { fontWeight: 500 })}>
        {CODE}
      </div>
      <div className="tr-pair__term">
        <span className="tr-tok-dim">$ </span>
        {typed(command, t, start, 34)}
        {t >= answered ? (
          <>
            {'\n'}
            <span className="tr-tok-ok" style={{ opacity: progress(t, answered, 0.2) }}>
              ✓ Paired with Northwind
            </span>
          </>
        ) : null}
      </div>
      <div className="tr-pair__status">
        <Swap
          from={
            <StatusChip icon="loader" spin tone="muted">
              Waiting for your runner…
            </StatusChip>
          }
          id="pair-status"
          inline
          k={progress(t, PAIRED, 0.3)}
          to={
            <StatusChip icon="circle-check" tone="success">
              Paired with leo-laptop
            </StatusChip>
          }
        />
      </div>
      <Arrive id="pair-runner" k={progress(t, PAIRED + 0.45, 0.45)}>
        <section aria-label="Runner" className="aw-card tr-runner">
          <header className="aw-card__head">
            <span className="tr-runner__icon">
              <Icon name={Laptop} size={16} />
            </span>
            <div className="aw-card__titles">
              <span className="aw-card__title">leo-laptop</span>
              <span className="aw-card__sub">Linux · 16 cores · paired just now</span>
            </div>
            <StatusChip tone="success">Online</StatusChip>
          </header>
          <Arrive id="pair-run" k={progress(t, PAIRED + 1.2, 0.45)}>
            <div className="aw-run__row">
              <Avatar kind="agent" name={agents.hermes.name} seed={agents.hermes.id} size={18} />
              <span>
                {agents.hermes.name} picked up <strong className="tr-runner__task">NW-24</strong> here
              </span>
              <span className="tr-runner__chip">
                <StatusChip icon="loader" spin tone="agent">
                  Working
                </StatusChip>
              </span>
            </div>
          </Arrive>
        </section>
      </Arrive>
    </div>
  )
}

/* A built-in agent */

const ASK = message(50, people.priya, '09:30', ['Turn the notes in #launch into a plan for 1.5.'])
const PLAN_TEXT = 'Here’s a plan for 1.5: six tasks on the board, each with an owner and a due date. Want me to share it in #launch?'
const PLAN = message(51, helper, '09:30', [PLAN_TEXT])
const YES = message(52, people.priya, '09:31', ['Yes please!'])
const SHARED_TEXT = 'Shared it in #launch and tagged each owner.'
const SHARED = message(53, helper, '09:31', [SHARED_TEXT])

/** One reply from the agent: it writes, then settles into the message it sent. */
function Reply({
  end,
  entry,
  extras,
  id,
  start,
  t,
  text,
}: {
  end: number
  entry: ReturnType<typeof message>
  extras?: ReactNode
  id: string
  start: number
  t: number
  text: string
}) {
  return (
    <Arrive id={id} k={progress(t, start, 0.35)}>
      <Swap
        from={<WritingRow actor={helper} k={(t - start) / (end - start - 0.1)} text={text} />}
        id={id}
        k={progress(t, end, 0.3)}
        to={<Message entry={entry} extras={extras} />}
      />
    </Arrive>
  )
}

function Builtin({ t }: { t: number }) {
  const ask = ARRIVE[2] + 0.55
  const stream = ask + 0.7
  const reply = stream + 1.4
  const yes = reply + 1.0
  const shared = yes + 0.55
  return (
    <Timeline pinned={false}>
      <Arrive id="next-ask" k={progress(t, ask, 0.4)}>
        <Message entry={ASK} />
      </Arrive>
      <Reply
        end={reply}
        entry={PLAN}
        extras={<Reactions id="next-plan" k={progress(t, reply + 0.55, 0.3)} reactions={[reaction('👍', '+1', [people.priya], true)]} />}
        id="next-plan"
        start={stream}
        t={t}
        text={PLAN_TEXT}
      />
      <Arrive id="next-yes" k={progress(t, yes, 0.4)}>
        <Message entry={YES} />
      </Arrive>
      <Reply end={shared + 0.9} entry={SHARED} id="next-shared" start={shared} t={t} text={SHARED_TEXT} />
    </Timeline>
  )
}

export function Next({ t }: { t: number }) {
  // The tiles come forward the whole time they're up.
  const push = mix(1, 1.03, progress(t, 0.4, 7, ease.linear))
  return (
    <div className="tr-layer">
      <Caption
        align="center"
        at={0.1}
        exitAt={EXIT}
        kicker="Coming next"
        kickerIcon={<Icon name={Sparkles} size={26} />}
        sub="Docs and sheets, any machine as a runner, and an agent that’s ready on day one."
        t={t}
        title={['Already in the works.']}
        titleScale={2.2}
        width={1500}
        x={210}
        y={140}
      />
      <div className="tr-layer" style={{ transform: `scale(${push})`, transformOrigin: '960px 614px' }}>
      <Tile desc="Write docs and sheets with your team and your agents, then send one to Google in a click." head={<TileTitle icon={FileText} title="Docs and sheets" />} index={0} t={t}>
        <Docs t={t} />
      </Tile>
      <Tile desc="Pair a laptop or a server with a short code. No public address needed." head={<TileTitle icon={Laptop} title="Pair any machine" />} index={1} t={t}>
        <Pair t={t} />
      </Tile>
      <Tile
        desc="An agent that works the moment you sign up, with no keys to set."
        head={
          <>
            <Avatar kind="agent" name={helper.name} seed={helper.id} size={24} />
            <span className="tr-tile__title">{helper.name}</span>
            <ActorTag kind="agent" />
          </>
        }
        index={2}
        t={t}
      >
        <Builtin t={t} />
      </Tile>
      </div>
    </div>
  )
}

function TileTitle({ icon, title }: { icon: typeof FileText; title: string }) {
  return (
    <>
      <Icon name={icon} size={18} />
      <span className="tr-tile__title">{title}</span>
    </>
  )
}
