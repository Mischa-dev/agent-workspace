import { useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react'

import { MessageReactions } from '../../../src/conversations/Reactions'
import { ReplyStatusList } from '../../../src/conversations/ReplyStatus'
import { ThreadChip } from '../../../src/conversations/ThreadChip'
import { MessageRow, StreamingMessageRow } from '../../../src/conversations/Timeline'
import type { ConversationTimelineEntry, MessageReaction, ReplyRequest } from '../../../src/conversations/types'
import { Paperclip, Type } from '../../../src/files/icons'
import { ActorTag, Avatar, AvatarStack, Dots, Icon, IconButton, Kbd, actorKind, formatShortcut } from '../../../src/ui'
import { conversationActor, viewer, type Actor } from '../data'
import { clamp, opening } from '../motion'

/*
 * A conversation as the app draws it: its header, the timeline of real
 * message rows (reactions, reply status, threads), and the composer with its
 * mention list. Messages arrive at the bottom and push the rest up.
 */

export function ChannelHeader({ members, name, topic }: { members: Actor[]; name: string; topic: string }) {
  return (
    <header className="aw-convhead">
      <div className="aw-convhead__main">
        <h1 className="aw-convhead__heading">
          <span className="aw-convhead__title aw-convhead__title--static">
            <Icon className="aw-convhead__glyph" name="hash" size={18} />
            <span className="aw-convhead__name">{name}</span>
            <Icon className="aw-convhead__caret" name="chevron-down" size={14} />
          </span>
        </h1>
        <div className="aw-convhead__topic">{topic}</div>
      </div>
      <div className="aw-convhead__actions">
        <AvatarStack max={5} people={members.map((member) => ({ id: member.id, name: member.name, kind: member.kind }))} size={22} />
        <IconButton icon="list" label="Threads" side="bottom" />
        <IconButton icon="messages" label="Start a thread" side="bottom" />
      </div>
    </header>
  )
}

const heights = new Map<string, number>()

/**
 * A message growing into the timeline: its row opens to full height while it
 * fades and rises in. The inner box is its own formatting context, so the
 * margins inside it are part of the height it's measured at, and the row ends
 * its opening exactly where it then stays: no jump on the last frame. `out`
 * takes it away again: it fades first, then its row closes. Both are
 * `progress` values on its default curve, which `opening` turns into the
 * row's height.
 */
export function Arrive({ children, id, k, out = 0, style }: { children: ReactNode; id: string; k: number; out?: number; style?: CSSProperties }) {
  const inner = useRef<HTMLDivElement>(null)
  const [, bump] = useState(0)
  useLayoutEffect(() => {
    const element = inner.current
    if (!element) return
    // The used height, unrounded and untouched by the window's scale.
    const height = parseFloat(getComputedStyle(element).height)
    if (heights.get(id) !== height) {
      heights.set(id, height)
      bump((n) => n + 1)
    }
  })
  if (k <= 0 || out >= 1) return null
  const height = heights.get(id)
  const done = k >= 1 && out <= 0
  const open = opening(k) * (1 - opening(out))
  return (
    <div data-arrive={id} style={{ height: done || height === undefined ? undefined : height * open, overflow: done ? undefined : 'hidden', ...style }}>
      <div ref={inner} style={{ display: 'flow-root', opacity: clamp(k * 1.4) * (1 - out), transform: `translate3d(0, ${(1 - k) * 16}px, 0)` }}>
        {children}
      </div>
    </div>
  )
}

/** The timeline: pinned to its newest message (a busy channel), or from the top (a new thread). */
export function Timeline({ children, pinned = true, style }: { children: ReactNode; pinned?: boolean; style?: CSSProperties }) {
  return (
    <div className="aw-timeline-wrap">
      <div className={pinned ? 'aw-timeline tr-timeline' : 'aw-timeline tr-timeline tr-timeline--top'} role="log" style={style}>
        {children}
      </div>
    </div>
  )
}

export function Day({ label }: { label: string }) {
  return (
    <div aria-label={label} className="aw-day" role="separator">
      <span>{label}</span>
    </div>
  )
}

const noop = () => undefined

/** One message row, with its reactions and whatever goes under it (reply status, a thread). */
export function Message({
  continued,
  entry,
  extras,
  reactions,
  highlighted,
}: {
  continued?: boolean
  entry: ConversationTimelineEntry
  extras?: ReactNode
  reactions?: MessageReaction[]
  highlighted?: boolean
}) {
  return (
    <MessageRow
      anchor={false}
      continued={continued}
      entry={entry}
      extras={extras}
      highlighted={highlighted}
      reactions={reactions?.length ? <MessageReactions reactions={reactions} viewerId={viewer.id} /> : null}
      viewerId={viewer.id}
    />
  )
}

export function reaction(emoji: string, name: string, by: Actor[], mine = false): MessageReaction {
  return { emoji, name, count: by.length, byViewer: mine, actors: by.map(conversationActor) }
}

/**
 * A message's reaction chips, opening into place the first time one arrives.
 * After that, `pops` animates single chips: one joining the row grows in, and
 * one whose count goes up bumps.
 */
export function Reactions({
  id,
  k = 1,
  pops = [],
  reactions,
}: {
  id: string
  k?: number
  pops?: { index: number; k: number; kind: 'join' | 'bump' }[]
  reactions: MessageReaction[]
}) {
  const css = pops
    .filter((pop) => pop.k > 0 && pop.k < 1)
    .map(({ index, k: shown, kind }) => {
      const chip = `.tr-rx-${id} > .aw-reactions > :nth-child(${index + 1})`
      if (kind === 'bump') return `${chip}{transform:scale(${1 + 0.16 * Math.sin(Math.PI * shown)})}`
      return `${chip}{opacity:${clamp(shown * 1.6)};transform:scale(${0.55 + 0.45 * (1 - (1 - shown) ** 3)})}`
    })
    .join('')
  return (
    <Arrive id={`${id}-reactions`} k={k}>
      <div className={`tr-rx-${id}`}>
        {css ? <style>{css}</style> : null}
        <MessageReactions reactions={reactions} viewerId={viewer.id} />
      </div>
    </Arrive>
  )
}

export function Replies({ requests }: { requests: ReplyRequest[] }) {
  return <ReplyStatusList onRetry={noop} requests={requests} />
}

export function request(id: string, agent: Actor, status: ReplyRequest['status'], progress?: string): ReplyRequest {
  const now = new Date().toISOString()
  return {
    id,
    messageId: id,
    agent: conversationActor(agent),
    required: false,
    status,
    reason: null,
    progress: progress ?? null,
    progressAt: null,
    updatedAt: now,
    agentLastSeenAt: now,
  }
}

export { StreamingMessageRow }

/**
 * An agent's reply while it writes, in the app's streaming row markup. The
 * whole reply is laid out from the first word, with the words not written yet
 * kept invisible, so the row never grows a line at a time and nothing above
 * it jumps; `k` is how much of it is written.
 */
export function WritingRow({ actor, k, text }: { actor: Actor; k: number; text: string }) {
  const words = text.split(' ')
  const count = Math.max(1, Math.floor(clamp(k) * words.length))
  return (
    <article aria-label={`${actor.name} is writing`} className="aw-msg is-streaming">
      <div className="aw-msg__gutter">
        <Avatar kind="agent" name={actor.name} seed={actor.id} size={36} working />
      </div>
      <div className="aw-msg__main">
        <div className="aw-msg__meta">
          <span className="aw-msg__author">{actor.name}</span>
          <ActorTag kind="agent" />
          <Dots label="Writing" />
        </div>
        <div className="aw-msg__body">
          <div className="aw-md aw-content">
            <p>
              {words.slice(0, count).join(' ')}
              <span style={{ opacity: 0 }}> {words.slice(count).join(' ')}</span>
            </p>
          </div>
        </div>
      </div>
    </article>
  )
}

/** The thread line under a message that opened one, as the app's own chip draws it. */
export function ThreadLine({ current, name, replies, working }: { current?: boolean; name: string; replies: number; working?: boolean }) {
  return (
    <ThreadChip
      current={current}
      onOpen={noop}
      thread={{ id: name, name, messageCount: replies, lastActivityAt: new Date().toISOString(), attention: working ? 'active' : undefined }}
    />
  )
}

/* The composer */

export type Suggestion = { actor: Actor }

// The kit's mention row height (conversation.css).
const MENTION_ROW = 34

function MentionRow({ active, actor }: { active: boolean; actor: Actor }) {
  return (
    <div aria-selected={active} className={active ? 'aw-mentions__item is-active' : 'aw-mentions__item'} role="option">
      <Avatar kind={actor.kind} name={actor.name} seed={actor.id} size={24} />
      <span className="aw-mentions__name">{actor.name}</span>
      <ActorTag kind={actorKind(actor.kind === 'agent' ? 'external_agent' : 'human')} />
      <span className="aw-mentions__handle">@{actor.handle}</span>
    </div>
  )
}

/**
 * The mention list over the composer. It opens on `k` and closes on `out`, on
 * a quick fade as a popover does, and while a query narrows it the rows that
 * stop matching fade and then fold away (their own `out`) instead of vanishing.
 * The highlight moves from `was` to `active` as `moved` runs from 0 to 1.
 */
export function MentionList({
  active,
  k = 1,
  moved = 1,
  out = 0,
  query,
  rows,
  was,
}: {
  active: string
  k?: number
  moved?: number
  out?: number
  query: string
  rows: { actor: Actor; k: number; out?: number }[]
  was?: string
}) {
  const lit = (id: string) => (id === active ? (was === active ? 1 : moved) : id === was ? 1 - moved : 0)
  if (out >= 1) return null
  return (
    <div
      aria-label="People and agents"
      className="aw-mentions"
      role="listbox"
      style={{ opacity: opening(k) * (1 - out), transform: `translate3d(0, ${(1 - k) * 8}px, 0) scale(${0.97 + 0.03 * k})`, transformOrigin: 'bottom left' }}
    >
      <div className="aw-mentions__head">
        People and agents
        {query ? ` matching “${query}”` : ''}
      </div>
      {rows.map(({ actor, k: shown, out = 0 }) =>
        shown <= 0 || out >= 1 ? null : (
          <div
            key={actor.id}
            style={shown < 1 || out > 0 ? { height: MENTION_ROW * opening(shown) * (1 - opening(out)), opacity: shown * (1 - out), overflow: 'hidden' } : undefined}
          >
            {lit(actor.id) >= 1 ? (
              <MentionRow active actor={actor} />
            ) : (
              <div style={{ position: 'relative' }}>
                <MentionRow active={false} actor={actor} />
                {lit(actor.id) > 0 ? (
                  <div aria-hidden="true" style={{ position: 'absolute', inset: 0, opacity: lit(actor.id) }}>
                    <MentionRow active actor={actor} />
                  </div>
                ) : null}
              </div>
            )}
          </div>
        ),
      )}
    </div>
  )
}

export function Composer({
  caret = true,
  list,
  placeholder,
  sending,
  text,
  typing,
}: {
  caret?: boolean
  list?: ReactNode
  placeholder: string
  sending?: boolean
  text: string
  /** Holds the caret steady while keys are going in, as a real caret does. */
  typing?: boolean
}) {
  return (
    <form className="aw-composer-wrap" onSubmit={(event) => event.preventDefault()}>
      <div className={text || caret ? 'aw-composer tr-focus' : 'aw-composer'}>
        {list}
        <div className={text ? 'tr-composer-text' : 'tr-composer-text is-empty'}>
          {text || (caret ? '' : placeholder)}
          {caret ? <span className={typing ? 'tr-caret is-typing' : 'tr-caret'} /> : null}
          {!text && caret ? <span className="tr-placeholder">{placeholder}</span> : null}
        </div>
        <div className="aw-composer__bar">
          <div className="aw-composer__tools">
            <IconButton icon={Paperclip} label="Attach files" size="sm" />
            <IconButton icon="at-sign" label="Mention someone" size="sm" />
            <IconButton icon="work" label="Reference a task" size="sm" />
            <IconButton icon={Type} label="Formatting" size="sm" />
          </div>
          <div className="aw-composer__end">
            <IconButton icon="clock" label="Schedule message" size="sm" />
            <button aria-label="Send" className={sending ? 'aw-composer__send tr-pressed' : 'aw-composer__send'} disabled={!text} type="submit">
              <Icon name="send" />
            </button>
          </div>
        </div>
      </div>
      <p className={text ? 'aw-composer__hint is-visible' : 'aw-composer__hint'}>
        <Kbd>{formatShortcut('enter')}</Kbd> to send · <Kbd>{formatShortcut('shift+enter')}</Kbd> for a new line
      </p>
    </form>
  )
}

/** The thread beside the channel: its name, the message it started from, and its replies. */
export function ThreadPanel({ children, name, origin }: { children: ReactNode; name: string; origin: ReactNode }) {
  return (
    <aside aria-label="Thread" className="context-drawer aw-threadpanel tr-panelfill">
      <div className="aw-threadpanel__head">
        <h2 className="aw-threadpanel__title">{name}</h2>
        <span className="aw-iconbtn aw-iconbtn--sm">
          <Icon name="external" size={16} />
        </span>
        <IconButton className="aw-panelclose" icon="x" label="Close thread" size="sm" />
      </div>
      <Timeline pinned={false}>
        <section aria-label="Started from" className="aw-origin">
          <p className="aw-origin__where">
            Started from a message in #launch · <span className="tr-link">View in channel</span>
          </p>
          {origin}
        </section>
        {children}
      </Timeline>
      <Composer caret={false} placeholder="Reply in thread" text="" />
    </aside>
  )
}
