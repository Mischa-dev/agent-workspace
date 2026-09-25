import { Fragment, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { useLocation } from 'react-router-dom'

import { ActorTag, Avatar, Icon, IconButton, actorKind, type IconName } from '../ui'
import { MessageContent } from './MessageContent'
import { dayKey, dayLabel, eventLabel, formatFullDate, formatTime } from './timeFormat'
import type { ConversationTimelineEntry, ConversationWorkerEvent } from './types'

const GROUP_WINDOW_MS = 5 * 60 * 1000

export function DayDivider({ label }: { label: string }) {
  return (
    <div aria-label={label} className="aw-day" role="separator">
      <span>{label}</span>
    </div>
  )
}

export function NewDivider() {
  return (
    <div aria-label="New messages" className="aw-newline" role="separator">
      <span>New</span>
    </div>
  )
}

/** The scrolling message list. Sticks to the newest message unless you've scrolled up to read. */
export function ConversationTimeline({
  busy = false,
  children,
  label,
  revision,
}: {
  busy?: boolean
  children: ReactNode
  label: string
  /** Changes when content is added, for example the highest sequence plus pending sends. */
  revision: string | number
}) {
  const scroller = useRef<HTMLDivElement>(null)
  const pinned = useRef(true)
  const location = useLocation()
  const [showLatest, setShowLatest] = useState(false)

  useLayoutEffect(() => {
    const element = scroller.current
    if (!element) return
    if (location.hash.startsWith('#message-')) return
    if (pinned.current) {
      element.scrollTop = element.scrollHeight
      setShowLatest(false)
    } else {
      setShowLatest(true)
    }
  }, [revision, location.hash])

  // Stay on the newest message when the pane resizes (a banner appears, the composer grows).
  useEffect(() => {
    const element = scroller.current
    if (!element || typeof ResizeObserver === 'undefined') return undefined
    const observer = new ResizeObserver(() => {
      if (pinned.current && !window.location.hash.startsWith('#message-')) element.scrollTop = element.scrollHeight
    })
    observer.observe(element)
    for (const child of Array.from(element.children)) observer.observe(child)
    return () => observer.disconnect()
  }, [revision])

  // A copied message link scrolls to that message and highlights it.
  useEffect(() => {
    if (!location.hash.startsWith('#message-')) return
    const target = document.getElementById(location.hash.slice(1))
    if (!target) return
    target.scrollIntoView({ block: 'center' })
    target.classList.add('is-linked')
    const timer = window.setTimeout(() => target.classList.remove('is-linked'), 2600)
    return () => window.clearTimeout(timer)
  }, [location.hash, revision])

  return (
    <div className="aw-timeline-wrap">
      <div
        aria-busy={busy}
        aria-label={label}
        className="aw-timeline"
        onScroll={(event) => {
          const element = event.currentTarget
          pinned.current = element.scrollHeight - element.scrollTop - element.clientHeight < 120
          if (pinned.current) setShowLatest(false)
        }}
        ref={scroller}
        role="log"
      >
        {children}
      </div>
      {showLatest ? (
        <button
          className="aw-latest"
          onClick={() => {
            const element = scroller.current
            if (!element) return
            element.scrollTo({ top: element.scrollHeight, behavior: 'smooth' })
            pinned.current = true
            setShowLatest(false)
          }}
          type="button"
        >
          <Icon name="arrow-down" size={14} />
          New messages
        </button>
      ) : null}
    </div>
  )
}

export function MessageRow({
  continued = false,
  entry,
  extras,
  highlighted = false,
  toolbar,
  viewerId,
}: {
  continued?: boolean
  entry: ConversationTimelineEntry
  extras?: ReactNode
  highlighted?: boolean
  toolbar?: ReactNode
  viewerId?: string
}) {
  const message = entry.message
  if (!message) return null
  const kind = actorKind(message.author.kind)
  const copyLink = () => {
    const url = `${window.location.origin}${window.location.pathname}#message-${message.id}`
    void navigator.clipboard?.writeText(url).catch(() => undefined)
  }
  return (
    <article
      aria-label={`${message.author.displayName}, ${formatTime(entry.createdAt)}`}
      className={['aw-msg', continued ? 'is-continued' : '', highlighted ? 'is-highlighted' : ''].filter(Boolean).join(' ')}
      id={`message-${message.id}`}
    >
      <div className="aw-msg__gutter">
        {continued ? (
          <time className="aw-msg__gutter-time" dateTime={entry.createdAt} title={formatFullDate(entry.createdAt)}>
            {formatTime(entry.createdAt).replace(/\s?[AP]M$/i, '')}
          </time>
        ) : (
          <Avatar kind={kind} name={message.author.displayName} seed={message.author.id} size={36} src={message.author.avatarUrl} />
        )}
      </div>
      <div className="aw-msg__main">
        {continued ? null : (
          <div className="aw-msg__meta">
            <span className="aw-msg__author">{message.author.displayName}</span>
            <ActorTag kind={kind} />
            <time className="aw-msg__time" dateTime={entry.createdAt} title={formatFullDate(entry.createdAt)}>
              {formatTime(entry.createdAt)}
            </time>
          </div>
        )}
        <div className="aw-msg__body">
          <MessageContent
            actors={message.mentionedActors ?? []}
            content={message.content}
            fallbackPlainText={message.plainText}
            viewerId={viewerId}
            workItems={message.referencedWorkItems ?? []}
          />
        </div>
        {extras}
      </div>
      <div aria-label="Message actions" className="aw-msg__toolbar" role="toolbar">
        {toolbar}
        <IconButton icon="link" label="Copy link to message" onClick={copyLink} size="sm" />
      </div>
    </article>
  )
}

export function ActivityRow({
  actor,
  children,
  detail,
  icon = 'terminal',
  time,
  tone,
}: {
  actor?: string
  children: ReactNode
  detail?: string
  icon?: IconName
  time?: string
  tone?: 'danger' | 'success' | 'warning'
}) {
  const [open, setOpen] = useState(false)
  return (
    <div className={tone ? `aw-event is-${tone}` : 'aw-event'}>
      <span className="aw-event__icon">
        <Icon name={icon} size={14} />
      </span>
      <div className="aw-event__main">
        <p className="aw-event__line">
          {actor ? <strong>{actor} </strong> : null}
          {children}
          {time ? <time> · {time}</time> : null}
          {detail ? (
            <button aria-expanded={open} className="aw-linkbtn aw-event__toggle" onClick={() => setOpen(!open)} type="button">
              {open ? 'Hide details' : 'Details'}
            </button>
          ) : null}
        </p>
        {open && detail ? <pre className="aw-code">{detail}</pre> : null}
      </div>
    </div>
  )
}

function eventIcon(kind: string): IconName {
  if (kind.includes('approval') || kind.includes('question')) return 'circle-help'
  if (kind.includes('failed')) return 'triangle-alert'
  if (kind.includes('interrupt') || kind.includes('cancel')) return 'stop'
  if (kind.includes('completed') || kind.includes('succeeded')) return 'circle-check'
  if (kind.includes('pull_request')) return 'pull-request'
  if (kind.includes('publish') || kind.includes('branch')) return 'branch'
  if (kind.includes('recovery')) return 'retry'
  return 'terminal'
}

function eventTone(kind: string) {
  if (kind.includes('failed')) return 'danger' as const
  if (kind.includes('completed')) return 'success' as const
  if (kind.includes('approval') || kind.includes('question') || kind.includes('blocked')) return 'warning' as const
  return undefined
}

export function WorkerEventRow({ createdAt, event }: { createdAt: string; event: ConversationWorkerEvent }) {
  return (
    <ActivityRow actor="Codex" icon={eventIcon(event.kind)} time={formatTime(createdAt)} tone={eventTone(event.kind)}>
      <span className="aw-event__kind">{eventLabel(event.kind)}.</span> {event.summary}
    </ActivityRow>
  )
}

export function SystemRow({ createdAt }: { createdAt: string }) {
  return (
    <ActivityRow icon="info" time={formatTime(createdAt)}>
      The conversation settings changed.
    </ActivityRow>
  )
}

/** Day dividers, the New line and Slack-style grouping for a run of timeline entries. */
export function TimelineEntries({
  entries,
  firstUnreadSequence,
  renderExtras,
  renderToolbar,
  viewerId,
}: {
  entries: ConversationTimelineEntry[]
  /** Show the New line before the first entry at or past this sequence. */
  firstUnreadSequence?: number | null
  renderExtras?: (entry: ConversationTimelineEntry) => ReactNode
  renderToolbar?: (entry: ConversationTimelineEntry) => ReactNode
  viewerId?: string
}) {
  const rows: ReactNode[] = []
  let previous: ConversationTimelineEntry | null = null
  let newShown = false
  for (const entry of entries) {
    const dividers: ReactNode[] = []
    if (!previous || dayKey(previous.createdAt) !== dayKey(entry.createdAt)) {
      dividers.push(<DayDivider key="day" label={dayLabel(entry.createdAt)} />)
    }
    if (!newShown && firstUnreadSequence != null && entry.sequence >= firstUnreadSequence && previous) {
      newShown = true
      dividers.push(<NewDivider key="new" />)
    }
    const continued = Boolean(
      entry.message &&
        previous?.message &&
        dividers.length === 0 &&
        previous.message.author.id === entry.message.author.id &&
        Date.parse(entry.createdAt) - Date.parse(previous.createdAt) < GROUP_WINDOW_MS,
    )
    previous = entry
    let row: ReactNode
    if (entry.message) {
      const highlighted = Boolean(viewerId && entry.message.mentionedActors?.some((actor) => actor.id === viewerId))
      row = (
        <MessageRow
          continued={continued}
          entry={entry}
          extras={renderExtras?.(entry)}
          highlighted={highlighted}
          toolbar={renderToolbar?.(entry)}
          viewerId={viewerId}
        />
      )
    } else if (entry.workerEvent) {
      row = <WorkerEventRow createdAt={entry.createdAt} event={entry.workerEvent} />
    } else {
      row = <SystemRow createdAt={entry.createdAt} />
    }
    rows.push(
      <Fragment key={entry.id}>
        {dividers}
        {row}
      </Fragment>,
    )
  }
  return <>{rows}</>
}

/** A message on its way to the server, shown dimmed until the timeline confirms it. */
export function PendingMessageRow({
  continued = false,
  name,
  seed,
  text,
}: {
  continued?: boolean
  name: string
  seed?: string
  text?: string
}) {
  return (
    <article aria-label="Message sending" className={continued ? 'aw-msg is-pending is-continued' : 'aw-msg is-pending'}>
      <div className="aw-msg__gutter">{continued ? null : <Avatar name={name} seed={seed} size={36} />}</div>
      <div className="aw-msg__main">
        {continued ? null : (
          <div className="aw-msg__meta">
            <span className="aw-msg__author">{name}</span>
            <span className="aw-msg__time">Sending…</span>
          </div>
        )}
        {text ? <div className="aw-msg__body">{text}</div> : <p className="aw-msg__note">Sending…</p>}
      </div>
    </article>
  )
}

export function OlderMessagesButton({ busy, onClick }: { busy: boolean; onClick: () => void }) {
  return (
    <div className="aw-older">
      <button className="aw-older__button" disabled={busy} onClick={onClick} type="button">
        {busy ? <Icon className="aw-spin" name="loader" size={14} /> : <Icon name="clock" size={14} />}
        {busy ? 'Loading earlier messages…' : 'Load earlier messages'}
      </button>
    </div>
  )
}
