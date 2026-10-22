import { Pin } from 'lucide-react'
import type { CSSProperties, ReactNode } from 'react'

import type { SavedItem, ScheduledMessage } from '../../../src/later/api'
import { SavedRow, ScheduledRow } from '../../../src/later/LaterRows'
import { ThreadStatusChip } from '../../../src/conversations/threadBoard'
import { ActorTag, Avatar, Button, Icon, Row, RowList, SegmentedControl, StatusChip, TextField, type IconName } from '../../../src/ui'
import { agents, message, people, project, type Actor } from '../data'
import { clamp, ease, mix, progress, typed } from '../motion'
import { Arrive, Composer, Message, Timeline, WritingRow } from '../parts/Chat'
import { Swap } from '../parts/Swap'
import { Caption } from '../parts/Text'

/*
 * 0:50. Everything a team chat does, each as the app draws it: the inbox, a
 * DM with an agent, Later, search, pins and the thread board. The tiles land
 * one after another, then a beat of each one working.
 */

const TILE = { w: 560, h: 330, gap: 28, x: 92, y: 296 }
const at = (column: number, row: number) => ({ x: TILE.x + column * (TILE.w + TILE.gap), y: TILE.y + row * (TILE.h + TILE.gap) })
const ARRIVE = [0.35, 0.5, 0.65, 0.8, 0.95, 1.1]
const EXIT = 6.75

const noop = () => undefined
const minutesAgo = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString()

function Tile({ children, head, index, t }: { children: ReactNode; head: ReactNode; index: number; t: number }) {
  const k = progress(t, ARRIVE[index], 0.6)
  const out = progress(t, EXIT + index * 0.04, 0.4, ease.inOut)
  const { x, y } = at(index % 3, Math.floor(index / 3))
  const style: CSSProperties = {
    left: x,
    top: y,
    width: TILE.w,
    height: TILE.h,
    opacity: k * (1 - out),
    transform: `translate3d(0, ${(1 - k) * 40 - out * 30}px, 0) scale(${mix(0.96, 1, k)})`,
  }
  return (
    <section className="tr-tile" data-theme="indigo" style={style}>
      <header className="tr-tile__head">{head}</header>
      <div className="tr-tile__body">{children}</div>
    </section>
  )
}

function TileTitle({ icon, title, children }: { icon: IconName | typeof Pin; title: string; children?: ReactNode }) {
  return (
    <>
      <Icon name={icon} size={18} />
      <span className="tr-tile__title">{title}</span>
      {children}
    </>
  )
}

/** Rows that arrive one after another from `start`. */
function arrival(t: number, start: number, index: number, step = 0.12): CSSProperties {
  const k = progress(t, start + index * step, 0.45)
  return { opacity: k, transform: `translate3d(0, ${(1 - k) * 12}px, 0)` }
}

/* Inbox */

type Notice = { actor?: Actor; icon?: IconName; title: string; preview: string; when: string; unread: boolean }

const NOTICES: Notice[] = [
  { actor: agents.muse, title: 'Muse needs your approval', preview: 'Add the Export button to the design kit?', when: 'now', unread: true },
  { actor: people.leo, title: 'Leo Martin mentioned you in #launch', preview: '@Priya Shah do the export columns look right to you?', when: '2m', unread: true },
  { icon: 'circle-check', title: 'Codex finished a run on NW-12', preview: 'Opened pull request #418. Tests pass.', when: '5m', unread: true },
  { actor: agents.grok, title: 'Grok assigned you NW-28', preview: 'Document the format', when: '1h', unread: false },
]

function Inbox({ t }: { t: number }) {
  return (
    <div className="aw-inbox__list tr-inbox">
      <section className="aw-inbox__group">
        <h2 className="aw-inbox__group-title">
          <span>Today</span>
          <span className="aw-inbox__group-project">{project.name}</span>
        </h2>
        {NOTICES.map((notice, index) => (
          <div className={notice.unread ? 'aw-inbox__row is-unread' : 'aw-inbox__row'} key={notice.title} role="listitem" style={arrival(t, ARRIVE[0] + 0.35, index)}>
            <span className="aw-inbox__row-main">
              <span aria-hidden="true" className="aw-inbox__dot" />
              {notice.actor ? (
                <Avatar kind={notice.actor.kind} name={notice.actor.name} seed={notice.actor.id} size={28} />
              ) : (
                <span className="aw-inbox__kind-icon">
                  <Icon name={notice.icon ?? 'info'} size={16} />
                </span>
              )}
              <span className="aw-inbox__row-text">
                <span className="aw-inbox__row-title">{notice.title}</span>
                <span className="aw-inbox__row-preview">{notice.preview}</span>
              </span>
              <time className="aw-inbox__time">{notice.when}</time>
            </span>
          </div>
        ))}
      </section>
    </div>
  )
}

/* A DM with an agent: Priya asks, and it answers as it writes. */

const ASK_TEXT = 'What changed in #launch this week?'
const ASK = message(30, people.priya, '09:12', [ASK_TEXT])
const SUMMARY_TEXT = 'Export is underway in NW-12, webhook retries are ready for review, and the board got faster on big projects.'
const SUMMARY = message(31, agents.claude, '09:12', [
  'Export is underway in ',
  { task: 12, state: 'started' },
  ', webhook retries are ready for review, and the board got faster on big projects.',
])

const DM = (() => {
  const typing = ARRIVE[1] + 0.4
  const send = typing + ASK_TEXT.length / 30 + 0.2
  const stream = send + 0.45
  return { typing, send, stream, reply: stream + 1.35 }
})()

function Direct({ t }: { t: number }) {
  const written = clamp((t - DM.stream) / (DM.reply - DM.stream - 0.1))
  return (
    <>
      <Timeline>
        {t >= DM.send ? (
          <Arrive id="dm-ask" k={progress(t, DM.send, 0.3)}>
            <Message entry={ASK} />
          </Arrive>
        ) : null}
        <Arrive id="dm-reply" k={progress(t, DM.stream, 0.35)}>
          <Swap
            from={<WritingRow actor={agents.claude} k={written} text={SUMMARY_TEXT} />}
            id="dm-reply"
            k={progress(t, DM.reply, 0.3)}
            to={<Message entry={SUMMARY} />}
          />
        </Arrive>
      </Timeline>
      <Composer
        placeholder={`Message ${agents.claude.name}`}
        sending={t >= DM.send - 0.12 && t < DM.send + 0.04}
        text={t < DM.send ? typed(ASK_TEXT, t, DM.typing, 30) : ''}
        typing={t >= DM.typing && t < DM.send}
      />
    </>
  )
}

/* Later: saved messages, reminders and scheduled messages. */

const NOW = new Date('2026-10-21T09:14:00')

const saved = (item: Pick<SavedItem, 'id' | 'kind'> & Partial<SavedItem>): SavedItem => ({
  note: '',
  remindAt: null,
  done: false,
  doneAt: null,
  createdAt: '2026-10-21T09:07:00',
  projectId: project.id,
  projectName: project.name,
  href: '#',
  message: null,
  workItem: null,
  ...item,
})

const SAVED: SavedItem[] = [
  saved({
    id: 's1',
    kind: 'message',
    remindAt: '2026-10-22T09:00:00',
    message: {
      id: 'm1',
      conversationId: 'c1',
      place: '#launch',
      authorName: 'Hermes',
      preview: 'Heads up for NW-12: big projects time out if the export loads every row at once.',
      removed: false,
      createdAt: '2026-10-21T09:06:00',
    },
  }),
  saved({ id: 's2', kind: 'reminder', note: 'Send the Acme team the 1.4 notes', remindAt: '2026-10-23T16:00:00' }),
  saved({ id: 's3', kind: 'work_item', workItem: { id: 'w19', key: 'NW-19', title: 'Import plans from Markdown', statusCategory: 'review' } }),
]

const scheduled = (id: string, place: string, text: string, sendAt: string): ScheduledMessage => ({
  id,
  conversationId: `c-${id}`,
  place,
  projectId: project.id,
  projectName: project.name,
  href: '#',
  content: { version: 1, nodes: [{ type: 'text', text }] } as ScheduledMessage['content'],
  plainText: text,
  sendAt,
  status: 'scheduled',
  failure: null,
  messageHref: null,
  createdAt: '2026-10-21T09:11:00',
})

const SCHEDULED = [
  scheduled('q1', '#general', 'Morning all! Release notes for 1.4 are ready for review.', '2026-10-22T09:00:00'),
  scheduled('q2', 'Leo Martin', 'Can we pair on the board filters on Friday?', '2026-10-23T10:30:00'),
]

const LATER_TABS = [
  { value: 'open', label: 'Saved' },
  { value: 'done', label: 'Done' },
  { value: 'scheduled', label: 'Scheduled' },
]

function Later({ t }: { t: number }) {
  const start = ARRIVE[2] + 0.35
  const flip = start + 2.3
  // The saved list lifts away as the scheduled one comes in under the new tab.
  const leave = progress(t, flip, 0.3, ease.inOut)
  return (
    <div className="tr-tile__pad">
      <div className="aw-later__tabs">
        <Swap
          from={<SegmentedControl label="Show" onChange={noop} options={LATER_TABS} value="open" />}
          id="later-tabs"
          inline
          k={progress(t, flip, 0.25)}
          to={<SegmentedControl label="Show" onChange={noop} options={LATER_TABS} value="scheduled" />}
        />
        {leave < 1 ? (
          <span className="aw-later__count" style={{ opacity: 1 - leave }}>
            3 items
          </span>
        ) : null}
      </div>
      <div style={{ position: 'relative' }}>
        {leave < 1 ? (
          <ul
            className="aw-later__list"
            style={leave > 0 ? { position: 'absolute', top: 0, left: 0, right: 0, opacity: 1 - leave, transform: `translate3d(0, ${-14 * leave}px, 0)` } : undefined}
          >
            {SAVED.map((item, index) => (
              <div className="tr-rise" key={item.id} style={arrival(t, start, index)}>
                <SavedRow busy={false} item={item} now={NOW} onDone={noop} onRemind={noop} onRemindCustom={noop} onRemove={noop} />
              </div>
            ))}
          </ul>
        ) : null}
        {leave > 0 ? (
          <ul className="aw-later__list">
            {SCHEDULED.map((item, index) => (
              <div className="tr-rise" key={item.id} style={arrival(t, flip + 0.12, index, 0.1)}>
                <ScheduledRow busy={false} item={item} now={NOW} onCancel={noop} onCopy={noop} onEdit={noop} onReschedule={noop} onSendNow={noop} />
              </div>
            ))}
          </ul>
        ) : null}
      </div>
    </div>
  )
}

/* Search across messages, threads, tasks, pages and decisions. */

type Hit = { kind: string; tone: 'agent' | 'muted'; status?: string; title: string; where: string; when: string; snippet: [string, string, string] }

const HITS: Hit[] = [
  {
    kind: 'Decision',
    tone: 'agent',
    title: 'CSV columns follow NW-9',
    where: '#launch · Leo Martin',
    when: minutesAgo(60 * 49),
    snippet: ['The ', 'export', ' uses key, title, status, assignee and due, in that order.'],
  },
  {
    kind: 'Task',
    tone: 'muted',
    status: 'Ready',
    title: 'NW-12 Export tasks as CSV',
    where: 'Priya Shah',
    when: minutesAgo(30),
    snippet: ['Let people download any task view as a CSV file, an ', 'export', ' that streams.'],
  },
]

function Search({ t }: { t: number }) {
  const start = ARRIVE[3] + 0.35
  const query = typed('export', t, start, 16)
  const results = start + 0.55
  return (
    <div className="tr-tile__pad">
      <form className="aw-inline-form aw-search__form" onSubmit={(event) => event.preventDefault()} role="search">
        <TextField aria-label="Search" placeholder="theming decision, AWD-12, “exact phrase”…" readOnly value={query} />
        <Button className={t >= results - 0.1 && t < results + 0.06 ? 'tr-pressed' : undefined} disabled={!query} icon="search" variant="primary">
          Search
        </Button>
      </form>
      <div className="aw-search__results">
        <RowList label="Results">
          {HITS.map((hit, index) =>
            t >= results + index * 0.12 ? (
              <div className="tr-rise" key={hit.title} style={arrival(t, results, index)}>
                <Row
                  className="aw-row--link"
                  meta={
                    <span className="aw-search__meta">
                      <StatusChip tone={hit.tone}>{hit.kind}</StatusChip>
                      {hit.status ? <StatusChip tone="muted">{hit.status}</StatusChip> : null}
                      <span>{hit.where}</span>
                    </span>
                  }
                  title={<span className="aw-row__link">{hit.title}</span>}
                >
                  <p className="aw-row__text aw-search__snippet">
                    {hit.snippet[0]}
                    <mark>{hit.snippet[1]}</mark>
                    {hit.snippet[2]}
                  </p>
                </Row>
              </div>
            ) : null,
          )}
        </RowList>
      </div>
    </div>
  )
}

/* Pins at the top of a channel: pages, decisions and messages. */

const PINS = [
  { title: '1.4 launch plan', text: 'Scope, dates and owners for the release' },
  { title: 'CSV columns follow NW-9', text: 'Decided by Leo Martin on Oct 19' },
  { title: 'Release checklist', text: 'Load test, update the docs, tag 1.4' },
]

function Pins({ t }: { t: number }) {
  const start = ARRIVE[4] + 0.5
  const open = t >= start
  return (
    <>
      <div className="aw-pins">
        <span aria-expanded={open} className={t >= start - 0.12 && t < start + 0.04 ? 'aw-pins__toggle tr-hover' : 'aw-pins__toggle'}>
          <Icon name={Pin} size={14} />
          3 pinned
          <Icon name="chevron-down" size={12} />
        </span>
        {open ? (
          <ul className="aw-pins__list">
            {PINS.map((pin, index) => (
              <li className="aw-pins__item" key={pin.title} style={arrival(t, start, index, 0.1)}>
                <span className="aw-pins__title tr-link">{pin.title}</span>
                <span className="aw-pins__text">{pin.text}</span>
              </li>
            ))}
          </ul>
        ) : null}
      </div>
    </>
  )
}

/* The thread board: each topic with its status and outcome. */

const THREADS = [
  { name: 'Export tasks as CSV', status: 'In progress', replies: '3 replies', when: 'just now' },
  { name: 'Column order for exports', status: 'Decided', replies: '12 replies', when: '2 days ago', outcome: 'Key, title, status, assignee, due.' },
  { name: 'Webhook retries', status: 'Blocked', done: 'Done', replies: '5 replies', when: '1 hr ago', outcome: 'Waiting on the load test.', doneOutcome: 'Load test passed with 10,000 tasks.' },
]

const SETTLE = 4.2

function Threads({ t }: { t: number }) {
  const start = ARRIVE[5] + 0.35
  const settled = progress(t, SETTLE, 0.35)
  // The row that settles lights up for a moment.
  const lit = progress(t, SETTLE, 0.3) * (1 - progress(t, SETTLE + 0.9, 0.9, ease.inOut))
  return (
    <div className="aw-threadboard tr-tile__pad">
      <SegmentedControl
        label="Show threads"
        onChange={noop}
        options={[
          { value: 'open', label: 'Open' },
          { value: 'settled', label: 'Settled' },
          { value: 'all', label: 'All' },
        ]}
        value="all"
      />
      <ul className="aw-threadboard__list">
        {THREADS.map((thread, index) => (
          <li key={thread.name} style={arrival(t, start, index)}>
            <span
              className="aw-threadboard__item"
              style={
                thread.done && lit > 0
                  ? {
                      background: `color-mix(in oklch, var(--success-soft) ${lit * 100}%, transparent)`,
                      borderColor: `color-mix(in oklch, var(--success) ${lit * 40}%, transparent)`,
                    }
                  : undefined
              }
            >
              <span className="aw-threadboard__top">
                <span className="aw-threadboard__name">{thread.name}</span>
              </span>
              <span className="aw-threadboard__meta">
                {thread.done ? (
                  <Swap from={<ThreadStatusChip status={thread.status} />} id={`thread-${index}-status`} inline k={settled} to={<ThreadStatusChip status={thread.done} />} />
                ) : (
                  <ThreadStatusChip status={thread.status} />
                )}
                <span>{thread.replies}</span>
                <span>· {thread.when}</span>
              </span>
              {thread.outcome ? (
                <span className="aw-threadboard__outcome">
                  {thread.doneOutcome ? <Swap from={thread.outcome} id={`thread-${index}-outcome`} k={settled} to={thread.doneOutcome} /> : thread.outcome}
                </span>
              ) : null}
            </span>
          </li>
        ))}
      </ul>
    </div>
  )
}

export function Features({ t }: { t: number }) {
  const push = mix(0.985, 1.015, progress(t, 0.3, 6.8, ease.linear))
  return (
    <div className="tr-layer">
      <Caption
        align="center"
        at={0.1}
        exitAt={EXIT}
        kicker="Team chat"
        kickerIcon={<Icon name="messages" size={26} />}
        sub="Channels, threads, DMs, an inbox, reminders, search and pins. Agents use them too."
        t={t}
        title={['Everything your team chat does.']}
        titleScale={2.2}
        width={1500}
        x={210}
        y={62}
      />
      <div className="tr-layer" style={{ transform: `scale(${push})`, transformOrigin: '960px 640px' }}>
        <Tile head={<TileTitle icon="inbox" title="Inbox" />} index={0} t={t}>
          <Inbox t={t} />
        </Tile>
        <Tile
          head={
            <>
              <Avatar kind="agent" name={agents.claude.name} seed={agents.claude.id} size={24} />
              <span className="tr-tile__title">{agents.claude.name}</span>
              <ActorTag kind="agent" />
            </>
          }
          index={1}
          t={t}
        >
          <Direct t={t} />
        </Tile>
        <Tile head={<TileTitle icon="bookmark" title="Later" />} index={2} t={t}>
          <Later t={t} />
        </Tile>
        <Tile head={<TileTitle icon="search" title="Search" />} index={3} t={t}>
          <Search t={t} />
        </Tile>
        <Tile
          head={
            <>
              <Icon name="hash" size={18} />
              <span className="tr-tile__title">launch</span>
              <span className="tr-tile__topic">Shipping 1.4 on Nov 14</span>
            </>
          }
          index={4}
          t={t}
        >
          <Pins t={t} />
        </Tile>
        <Tile head={<TileTitle icon="messages" title="Threads" />} index={5} t={t}>
          <Threads t={t} />
        </Tile>
      </div>
    </div>
  )
}
