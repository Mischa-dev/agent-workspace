import { Fragment, lazy, Suspense, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'

import { ActorTag, Avatar, Button, Dots, Icon, IconButton, Sheet, actorKind, countLabel, type IconName } from '../ui'
import { usePhoneWidth } from '../ui/useCoarsePointer'
import { Markdown } from '../ui/markdown/Markdown'
import { Attachments, LinkPreviews } from '../files/Attachments'
import type { FileAttachment } from '../files/model'
import { normalizeFiles, normalizeLinkPreviews } from '../files/model'
import './chat.css'
import type { ComposerSubmission } from './composerModel'
import { ForwardedQuote } from './ForwardedQuote'
import { ReplyQuote } from './ReplyQuote'
import { useLongPress } from './useLongPress'
import { useTyping } from '../realtime/realtimeContext'
import { MessageContent } from './MessageContent'
import { parseMessageContent } from './messageContentModel'
import { thingsLinkedIn } from '../modules/references/thingLinks'
import { dayKey, dayLabel, eventLabel, formatFullDate, formatTime } from './timeFormat'
import { formatDateTime } from '../ui/time'
import type {
  ConversationActor,
  ConversationMessage,
  ConversationSystemEvent,
  ConversationTimelineEntry,
  ConversationWorkItemChip,
  ConversationWorkerEvent,
  MessageContentV1,
  ReplyTarget,
} from './types'
import '../styles/conversation.css'

const GROUP_WINDOW_MS = 5 * 60 * 1000
/** Within this many pixels of the bottom counts as reading the newest message. */
const PINNED_SLACK = 120
/** Scrolling up to within this many pixels of the top loads earlier messages. */
const NEAR_TOP = 300

const ProfileDialog = lazy(() => import('../profiles/ProfileDialog'))
const LinkCards = lazy(() => import('../modules/references/LinkCards'))
const MadeThing = lazy(() => import('../modules/references/MadeThing'))
const MadeIntoChips = lazy(() => import('../modules/references/MadeThing').then((module) => ({ default: module.MadeIntoChips })))

/** A message's own words, with its links, for finding the things it links to. */
function linkedThings(message: ConversationMessage) {
  const parsed = parseMessageContent(message.content)
  const text = parsed ? parsed.nodes.map((node) => (node.type === 'text' ? node.text : ' ')).join('') : message.plainText
  return thingsLinkedIn(text ?? '')
}

function prefersReducedMotion() {
  return typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches
}

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

/** Where unread starts when a conversation opens, for the "new messages" bar. */
export type UnreadSummary = {
  /** Unread messages from others among those loaded. */
  count: number
  /** Unread goes back past the loaded messages. */
  more: boolean
  /** When the first unread message was sent, if it's loaded. */
  since: string | null
}

function unreadLabel(unread: UnreadSummary) {
  const count = unread.more ? `${unread.count}+ new messages` : countLabel(unread.count, 'new message')
  if (!unread.since) return count
  const sameDay = new Date(unread.since).toDateString() === new Date().toDateString()
  return `${count} since ${sameDay ? formatTime(unread.since) : formatDateTime(unread.since)}`
}

/**
 * The scrolling message list. Sticks to the newest message unless you've
 * scrolled up to read, keeps your place when earlier messages load above, and
 * scrolls a linked message (`#message-…`) into view, loading earlier pages to
 * find it.
 */
export function ConversationTimeline({
  busy = false,
  children,
  followLinks = true,
  hasOlder = false,
  label,
  olderKey = 0,
  onLoadOlder,
  revision,
  stickKey,
  unread = null,
}: {
  busy?: boolean
  children: ReactNode
  /** Answer `#message-…` links. Only the page's main list does, not a thread beside it. */
  followLinks?: boolean
  /** There are earlier messages to load. */
  hasOlder?: boolean
  label: string
  /** Changes when earlier messages are added above, such as how many are loaded. */
  olderKey?: number
  /** Loads the page before the oldest shown; called near the top and to find a linked message. */
  onLoadOlder?: () => void
  /** Changes when content is added at the bottom, for example the highest sequence plus pending sends. */
  revision: string | number
  /** Changes when you send: the list goes back to the newest message. */
  stickKey?: string | number
  /** Unread when the conversation opened; shows a bar to jump up to the New line while it's out of sight. */
  unread?: UnreadSummary | null
}) {
  const scroller = useRef<HTMLDivElement>(null)
  const pinned = useRef(true)
  const location = useLocation()
  const navigate = useNavigate()
  const [showLatest, setShowLatest] = useState(false)
  const [jumpHidden, setJumpHidden] = useState(false)
  const [newlineAbove, setNewlineAbove] = useState(false)
  // Where the list stood after the last render, to keep your place when older messages load.
  const last = useRef({ height: 0, top: 0, olderKey })
  const lastTop = useRef(0)
  const linking = followLinks && location.hash.startsWith('#message-')

  // Another conversation opens with its header showing (C2.31).
  useEffect(() => {
    const page = followLinks ? scroller.current?.closest('main') : null
    if (page) delete page.dataset.headFolded
    return () => {
      if (page) delete page.dataset.headFolded
    }
  }, [followLinks, label])

  // Earlier messages went in above: move down by what they added, so the
  // message you were reading stays put.
  useLayoutEffect(() => {
    const element = scroller.current
    if (!element) return
    if (last.current.olderKey !== olderKey && !pinned.current) {
      element.scrollTop = last.current.top + (element.scrollHeight - last.current.height)
    }
    last.current = { height: element.scrollHeight, top: element.scrollTop, olderKey }
  })

  useLayoutEffect(() => {
    const element = scroller.current
    if (!element || linking) return
    if (pinned.current) {
      element.scrollTop = element.scrollHeight
      setShowLatest(false)
    } else {
      setShowLatest(true)
    }
    // Only new content at the bottom moves the list, never older pages.
  }, [revision, linking])

  // Your own message goes to the bottom, wherever you were reading.
  useLayoutEffect(() => {
    const element = scroller.current
    if (!element || stickKey === undefined) return
    pinned.current = true
    element.scrollTop = element.scrollHeight
    setShowLatest(false)
  }, [stickKey])

  // Stay on the newest message when the pane resizes (a banner appears, the composer grows).
  useEffect(() => {
    const element = scroller.current
    if (!element || typeof ResizeObserver === 'undefined') return undefined
    const stick = () => {
      if (pinned.current && !window.location.hash.startsWith('#message-')) element.scrollTop = element.scrollHeight
      last.current = { ...last.current, height: element.scrollHeight, top: element.scrollTop }
    }
    const observer = new ResizeObserver(stick)
    const watchChildren = () => {
      for (const child of Array.from(element.children)) observer.observe(child)
    }
    observer.observe(element)
    watchChildren()
    // Parts that arrive on their own (a thread's first message loads
    // separately) are new children: watch them too, or the list stays at the
    // top while they push the newest message out of sight.
    const added =
      typeof MutationObserver === 'undefined'
        ? null
        : new MutationObserver(() => {
            watchChildren()
            stick()
          })
    added?.observe(element, { childList: true })
    return () => {
      observer.disconnect()
      added?.disconnect()
    }
  }, [revision, olderKey])

  // A copied message link scrolls to that message and highlights it, loading
  // earlier pages until it's there. The link then leaves the address, so new
  // messages are followed again.
  useEffect(() => {
    if (!linking) return
    const id = location.hash.slice(1)
    const found = document.getElementById(id)
    const target = found && scroller.current?.contains(found) ? found : null
    if (!target) {
      if (busy) return
      if (hasOlder && onLoadOlder) onLoadOlder()
      // Not here at all (removed, or never in this conversation): stop looking.
      else navigate({ pathname: location.pathname, search: location.search }, { replace: true, preventScrollReset: true })
      return
    }
    pinned.current = false
    target.scrollIntoView({ block: 'center' })
    target.classList.add('is-linked')
    window.setTimeout(() => target.classList.remove('is-linked'), 2600)
    navigate({ pathname: location.pathname, search: location.search }, { replace: true, preventScrollReset: true })
  }, [busy, hasOlder, linking, location.hash, location.pathname, location.search, navigate, olderKey, onLoadOlder, revision])

  // Whether the New line is above what's on screen, for the jump bar.
  const checkNewline = () => {
    const element = scroller.current
    const line = element?.querySelector<HTMLElement>('.aw-newline')
    if (!element || !line) {
      setNewlineAbove(false)
      return
    }
    const above = line.getBoundingClientRect().bottom < element.getBoundingClientRect().top + 8
    setNewlineAbove(above)
    // Once you've seen it, the bar has done its job.
    if (!above) setJumpHidden(true)
  }
  useLayoutEffect(() => {
    if (unread && !jumpHidden) checkNewline()
    // Re-check as the list settles on the newest message.
  }, [unread, jumpHidden, revision])

  const jumpToNew = () => {
    const element = scroller.current
    const line = element?.querySelector<HTMLElement>('.aw-newline')
    setJumpHidden(true)
    if (!element) return
    if (!line) {
      element.scrollTo({ top: 0, behavior: prefersReducedMotion() ? 'auto' : 'smooth' })
      return
    }
    pinned.current = false
    line.scrollIntoView({ block: 'start', behavior: prefersReducedMotion() ? 'auto' : 'smooth' })
  }

  return (
    <div className="aw-timeline-wrap">
      {unread && unread.count > 0 && newlineAbove && !jumpHidden ? (
        <div className="aw-jumpbar" role="status">
          <span className="aw-jumpbar__text">{unreadLabel(unread)}</span>
          <Button icon="arrow-up" onClick={jumpToNew} size="sm" variant="ghost">
            Jump
          </Button>
          <IconButton icon="x" label="Hide" onClick={() => setJumpHidden(true)} size="sm" tooltip={false} />
        </div>
      ) : null}
      <div
        aria-busy={busy}
        aria-label={label}
        className="aw-timeline"
        onScroll={(event) => {
          const element = event.currentTarget
          pinned.current = element.scrollHeight - element.scrollTop - element.clientHeight < PINNED_SLACK
          if (pinned.current) setShowLatest(false)
          const movingUp = element.scrollTop < lastTop.current
          // C2.31: on a phone the conversation's header folds away while you read back, and returns as you come down.
          const page = followLinks ? element.closest('main') : null
          if (page && element.scrollTop !== lastTop.current) page.dataset.headFolded = movingUp && !pinned.current ? 'true' : 'false'
          lastTop.current = element.scrollTop
          last.current = { ...last.current, top: element.scrollTop }
          if (movingUp && element.scrollTop < NEAR_TOP && hasOlder && onLoadOlder) onLoadOlder()
          if (unread && !jumpHidden) checkNewline()
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
            element.scrollTo({ top: element.scrollHeight, behavior: prefersReducedMotion() ? 'auto' : 'smooth' })
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

/** "This message was removed." or, when someone else removed it, who did. */
function removedLabel(message: ConversationMessage) {
  const remover = message.removed?.removedBy
  return remover && remover.id !== message.author.id ? `Removed by ${remover.displayName}.` : 'This message was removed.'
}

function EditedMark({ message, onOpenHistory }: { message: ConversationMessage; onOpenHistory?: (message: ConversationMessage) => void }) {
  const title = message.editedAt ? `Edited ${formatFullDate(message.editedAt)}` : 'Edited'
  if (!onOpenHistory) {
    return (
      <span className="aw-msg__edited" title={title}>
        (edited)
      </span>
    )
  }
  return (
    <button
      aria-label={`${title}. Show earlier versions`}
      className="aw-msg__edited"
      onClick={() => onOpenHistory(message)}
      title={title}
      type="button"
    >
      (edited)
    </button>
  )
}

export function MessageRow({
  anchor = true,
  continued = false,
  conversationId,
  editor,
  entry,
  extras,
  highlighted = false,
  onOpenHistory,
  reactions,
  toolbar,
  viewerId,
}: {
  /** Links to the message land here. Off for a copy shown elsewhere, such as atop its thread. */
  anchor?: boolean
  continued?: boolean
  /** Where the message is, for hiding one of its link previews. */
  conversationId?: string
  /** Replaces the body while the message is being edited. */
  editor?: ReactNode
  entry: ConversationTimelineEntry
  extras?: ReactNode
  highlighted?: boolean
  /** Opens the message's earlier versions from its "(edited)" mark. */
  onOpenHistory?: (message: ConversationMessage) => void
  /** The reaction chips, shown under the body (never for a removed message). */
  reactions?: ReactNode
  /** The hover actions; none for a message shown read-only. */
  toolbar?: ReactNode
  viewerId?: string
}) {
  const [profileOpen, setProfileOpen] = useState(false)
  // C2.31: on a phone, holding a message opens its actions in a bottom sheet.
  const phone = usePhoneWidth()
  const [sheetOpen, setSheetOpen] = useState(false)
  const sheeted = phone && Boolean(toolbar) && !editor
  const press = useLongPress(sheeted ? () => setSheetOpen(true) : undefined)
  const message = entry.message
  if (!message) return null
  const kind = actorKind(message.author.kind)
  const removed = Boolean(message.removed)
  const edited = !removed && (message.revision > 1 || Boolean(message.editedAt))
  const linked = removed ? [] : linkedThings(message)
  return (
    <article
      aria-label={`${message.author.displayName}, ${formatTime(entry.createdAt)}`}
      className={[
        'aw-msg',
        continued ? 'is-continued' : '',
        highlighted && !removed ? 'is-highlighted' : '',
        removed ? 'is-removed' : '',
        editor ? 'is-editing' : '',
      ]
        .filter(Boolean)
        .join(' ')}
      id={anchor ? `message-${message.id}` : undefined}
      // Focusable by tap, not Tab: touch screens have no hover, so tapping a
      // message is how its toolbar appears (see .aw-msg:focus-within).
      tabIndex={-1}
      {...press}
      onContextMenu={sheeted ? (event) => event.preventDefault() : undefined}
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
            <span className="aw-msg__author">
              {/* C2.17: the name opens their profile. */}
              <button
                aria-haspopup="dialog"
                className="aw-msg__author-button"
                onClick={() => setProfileOpen(true)}
                type="button"
              >
                {message.author.displayName}
              </button>
            </span>
            <ActorTag kind={kind} />
            {message.via ? <span className="aw-msg__via">via {message.via}</span> : null}
            {message.saved && !removed ? <Icon className="aw-msg__saved" label="Saved for later" name="bookmark" size={14} /> : null}
            <time className="aw-msg__time" dateTime={entry.createdAt} title={formatFullDate(entry.createdAt)}>
              {formatTime(entry.createdAt)}
            </time>
            {edited ? <EditedMark message={message} onOpenHistory={onOpenHistory} /> : null}
          </div>
        )}
        {removed ? (
          <p className="aw-msg__removed">{removedLabel(message)}</p>
        ) : editor ? (
          editor
        ) : (
          <div className="aw-msg__body">
            {message.replyTo ? <ReplyQuote value={message.replyTo} /> : null}
            <MessageContent
              actors={message.mentionedActors ?? []}
              content={message.content}
              fallbackPlainText={message.plainText}
              viewerId={viewerId}
              workItems={message.referencedWorkItems ?? []}
            />
            {continued && edited ? (
              <>
                {' '}
                <EditedMark message={message} onOpenHistory={onOpenHistory} />
              </>
            ) : null}
            {message.attachments?.length ? <Attachments files={normalizeFiles(message.attachments)} /> : null}
            {message.linkPreviews?.length ? (
              <LinkPreviews
                canHide={Boolean(viewerId) && viewerId === message.author.id}
                conversationId={conversationId}
                messageId={message.id}
                previews={normalizeLinkPreviews(message.linkPreviews)}
              />
            ) : null}
            {/* C2.20: what this message was made into, as the reader may see it. */}
            {message.madeInto?.length ? (
              <Suspense fallback={null}>
                <MadeIntoChips things={message.madeInto} />
              </Suspense>
            ) : null}
            {/* C2.23: a card for each thing in this app it links to, as the reader may see it. */}
            {linked.length ? (
              <Suspense fallback={null}>
                <LinkCards addresses={linked} />
              </Suspense>
            ) : null}
          </div>
        )}
        {removed || editor || !message.forwarded ? null : <ForwardedQuote value={message.forwarded} />}
        {profileOpen ? (
          <Suspense fallback={null}>
            <ProfileDialog accountId={message.author.id} name={message.author.displayName} onClose={() => setProfileOpen(false)} />
          </Suspense>
        ) : null}
        {removed || editor ? null : reactions}
        {extras}
      </div>
      {editor || !toolbar || (sheeted && sheetOpen) ? null : (
        <div aria-label="Message actions" className="aw-msg__toolbar" role="toolbar">
          {toolbar}
        </div>
      )}
      {sheeted && sheetOpen ? (
        <Sheet label="Message actions" onClose={() => setSheetOpen(false)}>
          <div
            aria-label="Message actions"
            className="aw-msg__sheet-actions"
            onClickCapture={(event) => {
              // An action that opens a menu keeps the sheet; any other closes it once done.
              const button = (event.target as HTMLElement).closest('button')
              if (button && !button.hasAttribute('aria-haspopup')) window.setTimeout(() => setSheetOpen(false), 0)
            }}
            role="toolbar"
          >
            {toolbar}
          </div>
        </Sheet>
      ) : null}
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

export function WorkerEventRow({ actor = 'Agent', createdAt, event }: { actor?: string; createdAt: string; event: ConversationWorkerEvent }) {
  return (
    <ActivityRow actor={actor} icon={eventIcon(event.kind)} time={formatTime(createdAt)} tone={eventTone(event.kind)}>
      <span className="aw-event__kind">{eventLabel(event.kind)}.</span> {event.summary}
    </ActivityRow>
  )
}

export function SystemRow({ createdAt, event }: { createdAt: string; event?: ConversationSystemEvent | null }) {
  if (event?.type === 'made_into') {
    const word = event.thingKind.replace(/_/g, ' ')
    return (
      <ActivityRow actor={event.actorName ?? 'Someone'} icon="link" time={formatTime(createdAt)}>
        made {event.messageId ? 'a message here' : 'this'} into{' '}
        <Suspense fallback={`a ${word}`}>
          <MadeThing id={event.thingId} kind={event.thingKind} />
        </Suspense>
      </ActivityRow>
    )
  }
  return (
    <ActivityRow icon="info" time={formatTime(createdAt)}>
      The conversation settings changed.
    </ActivityRow>
  )
}

/** "Mara", "Mara and Rook", "Mara, Rook and 2 others". */
function namesLabel(names: readonly string[]) {
  if (names.length <= 2) return names.join(' and ')
  return `${names[0]}, ${names[1]} and ${countLabel(names.length - 2, 'other')}`
}

/**
 * A run of working notes (AWD-220), folded to one line: how many, who wrote
 * them, when, and the start of the newest. Opening it shows the notes as
 * they were written. A link to one of them opens the fold.
 */
function NotesFold({ children, entries }: { children: ReactNode; entries: readonly ConversationTimelineEntry[] }) {
  const location = useLocation()
  const linked = entries.some((entry) => entry.message && location.hash === `#message-${entry.message.id}`)
  const [open, setOpen] = useState(linked)
  if (linked && !open) setOpen(true)
  const first = entries[0]
  const last = entries[entries.length - 1]
  const names = [...new Set(entries.map((entry) => entry.message?.author.displayName ?? ''))].filter(Boolean)
  const from = formatTime(first.createdAt)
  const to = formatTime(last.createdAt)
  const preview = last.message?.plainText.trim().split('\n')[0] ?? ''
  return (
    <div className={open ? 'aw-notes is-open' : 'aw-notes'} data-part="working-notes">
      <button aria-expanded={open} className="aw-notes__toggle" onClick={() => setOpen(!open)} type="button">
        <span className="aw-notes__icon">
          <Icon name={open ? 'chevron-down' : 'chevron-right'} size={14} />
        </span>
        <span className="aw-notes__line">
          <span className="aw-notes__count">{countLabel(entries.length, 'working note')}</span>
          <span className="aw-notes__meta">
            {' · '}
            {namesLabel(names)}
            {' · '}
            <time dateTime={first.createdAt} title={formatFullDate(first.createdAt)}>
              {from === to ? from : `${from} to ${to}`}
            </time>
          </span>
          {open || !preview ? null : <span className="aw-notes__preview">{preview}</span>}
        </span>
      </button>
      {open ? <div className="aw-notes__list">{children}</div> : null}
    </div>
  )
}

/** Day dividers, the New line and Slack-style grouping for a run of timeline entries. */
export function TimelineEntries({
  agentName,
  conversationId,
  entries,
  firstUnreadSequence,
  foldNotes = true,
  onOpenHistory,
  renderEditor,
  renderExtras,
  renderReactions,
  renderToolbar,
  viewerId,
}: {
  /** The name of the agent behind a run's events, by run id. */
  agentName?: (agentRunId: string) => string
  /** Where the entries are, for actions on a message such as hiding a link preview. */
  conversationId?: string
  entries: ConversationTimelineEntry[]
  /** Show the New line before the first entry at or past this sequence (above them all when unread starts earlier). */
  firstUnreadSequence?: number | null
  /**
   * Fold runs of working notes to one line each. Off in a working channel,
   * where notes are what you came to read. Your own notes, and notes that
   * mention you, are never folded.
   */
  foldNotes?: boolean
  onOpenHistory?: (message: ConversationMessage) => void
  /** The inline editor for the message being edited, if any. */
  renderEditor?: (entry: ConversationTimelineEntry) => ReactNode
  renderExtras?: (entry: ConversationTimelineEntry) => ReactNode
  /** The reaction chips for a message. */
  renderReactions?: (entry: ConversationTimelineEntry) => ReactNode
  renderToolbar?: (entry: ConversationTimelineEntry) => ReactNode
  viewerId?: string
}) {
  const rows: ReactNode[] = []
  let previous: ConversationTimelineEntry | null = null
  let previousFolded = false
  let newShown = false
  // The run of notes being folded, and its rows.
  let run: { entries: ConversationTimelineEntry[]; rows: ReactNode[] } | null = null
  const closeRun = () => {
    if (!run) return
    rows.push(
      <NotesFold entries={run.entries} key={`notes-${run.entries[0].id}`}>
        {run.rows}
      </NotesFold>,
    )
    run = null
  }
  for (const entry of entries) {
    const dividers: ReactNode[] = []
    if (!previous || dayKey(previous.createdAt) !== dayKey(entry.createdAt)) {
      dividers.push(<DayDivider key="day" label={dayLabel(entry.createdAt)} />)
    }
    // What you wrote yourself is never new to you, even when it was sent from
    // somewhere that didn't mark the conversation read (a notification reply).
    const own = Boolean(viewerId && entry.message?.author.id === viewerId)
    // A note that doesn't name you: it never made the conversation unread.
    const quiet = Boolean(
      entry.message?.note &&
        !entry.message.removed &&
        !own &&
        !(viewerId && entry.message.mentionedActors?.some((actor) => actor.id === viewerId)),
    )
    const folded = foldNotes && quiet
    // So it is never what's new, folded or not (a working channel shows its notes in full).
    if (!newShown && firstUnreadSequence != null && entry.sequence >= firstUnreadSequence && !own && !quiet) {
      newShown = true
      dividers.push(<NewDivider key="new" />)
    }
    const continued = Boolean(
      entry.message &&
        previous?.message &&
        dividers.length === 0 &&
        previousFolded === folded &&
        previous.message.author.id === entry.message.author.id &&
        previous.message.via === entry.message.via &&
        Date.parse(entry.createdAt) - Date.parse(previous.createdAt) < GROUP_WINDOW_MS,
    )
    previous = entry
    previousFolded = folded
    let row: ReactNode
    if (entry.message) {
      const highlighted = Boolean(viewerId && entry.message.mentionedActors?.some((actor) => actor.id === viewerId))
      row = (
        <MessageRow
          continued={continued}
          conversationId={conversationId}
          editor={renderEditor?.(entry)}
          entry={entry}
          extras={renderExtras?.(entry)}
          highlighted={highlighted}
          onOpenHistory={onOpenHistory}
          reactions={renderReactions?.(entry)}
          toolbar={renderToolbar?.(entry)}
          viewerId={viewerId}
        />
      )
    } else if (entry.workerEvent) {
      row = <WorkerEventRow actor={agentName?.(entry.workerEvent.agentRunId)} createdAt={entry.createdAt} event={entry.workerEvent} />
    } else {
      row = <SystemRow createdAt={entry.createdAt} event={entry.system} />
    }
    if (folded) {
      // A day or New line ends the run: it stays where it belongs.
      if (dividers.length) {
        closeRun()
        rows.push(<Fragment key={`dividers-${entry.id}`}>{dividers}</Fragment>)
      }
      run ??= { entries: [], rows: [] }
      run.entries.push(entry)
      run.rows.push(<Fragment key={entry.id}>{row}</Fragment>)
      continue
    }
    closeRun()
    rows.push(
      <Fragment key={entry.id}>
        {dividers}
        {row}
      </Fragment>,
    )
  }
  closeRun()
  return <>{rows}</>
}

/** A message on its way to the server, shown dimmed and formatted until the timeline has it. */
export function PendingMessageRow({
  actors = [],
  attachments,
  content,
  continued = false,
  name,
  replyTo,
  seed,
  text,
  workItems = [],
}: {
  actors?: ConversationActor[]
  attachments?: FileAttachment[]
  content?: MessageContentV1
  continued?: boolean
  name: string
  replyTo?: ReplyTarget
  seed?: string
  text?: string
  workItems?: ConversationWorkItemChip[]
}) {
  const hasText = Boolean(text?.trim())
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
        <div className="aw-msg__body">
          {replyTo ? (
            <ReplyQuote value={{ messageId: replyTo.messageId, available: true, authorName: replyTo.authorName, text: replyTo.text }} />
          ) : null}
          {hasText ? (
            <MessageContent actors={actors} content={content} fallbackPlainText={text ?? ''} workItems={workItems} />
          ) : null}
          {attachments?.length ? <Attachments files={attachments} /> : null}
        </div>
      </div>
    </article>
  )
}

/** Whether your next message joins your last one, the way the timeline groups them. */
function continuesFrom(entries: readonly ConversationTimelineEntry[], authorId: string, now = Date.now()) {
  const last = entries.at(-1)
  return Boolean(
    last?.message &&
      last.message.author.id === authorId &&
      !last.message.via &&
      dayKey(last.createdAt) === dayKey(new Date(now).toISOString()) &&
      now - Date.parse(last.createdAt) < GROUP_WINDOW_MS,
  )
}

/** Your messages on their way, after the timeline they'll join. */
export function PendingMessages({
  entries,
  items,
  viewer,
}: {
  entries: readonly ConversationTimelineEntry[]
  items: readonly ComposerSubmission[]
  viewer: { actorId: string; displayName: string }
}) {
  if (!items.length) return null
  const joins = continuesFrom(entries, viewer.actorId)
  return (
    <>
      {items.map((item, index) => (
        <PendingMessageRow
          actors={item.mentions}
          attachments={item.attachments}
          content={item.content}
          continued={index > 0 || joins}
          key={item.idempotencyKey}
          name={viewer.displayName}
          replyTo={item.replyTo}
          seed={viewer.actorId}
          text={item.text}
          workItems={item.workItems}
        />
      ))}
    </>
  )
}

/** The name a timeline gives an account, from the last message it wrote there. */
function nameIn(entries: readonly ConversationTimelineEntry[], actorId: string) {
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const author = entries[index].message?.author
    if (author?.id === actorId) return author.displayName
  }
  return null
}

/**
 * Who else is writing a message here now (W1.6), named from the timeline.
 * The line is always there, empty when nobody is, so screen readers hear it
 * change.
 */
export function TypingNow({
  conversationId,
  entries,
  viewerId,
}: {
  conversationId: string
  entries: readonly ConversationTimelineEntry[]
  viewerId: string
}) {
  const typing = useTyping(conversationId)
  const others = Object.keys(typing).filter((id) => id !== viewerId)
  const names = others.map((id) => nameIn(entries, id)).filter((name): name is string => name !== null)
  return (
    <div aria-live="polite" className="aw-typing" data-part="conversation-typing">
      {others.length ? (
        <>
          <Dots />
          <span>{typingText(others.length, names)}</span>
        </>
      ) : null}
    </div>
  )
}

/** "Dana is typing", with names the timeline knows and "someone" for the rest. */
function typingText(count: number, names: readonly string[]) {
  if (count >= 3) return 'Several people are typing'
  if (count === 2) {
    if (names.length === 2) return `${names[0]} and ${names[1]} are typing`
    return names.length === 1 ? `${names[0]} and someone else are typing` : 'Two people are typing'
  }
  return `${names[0] ?? 'Someone'} is typing`
}

/** What an agent is writing right now, formatted as it streams in. */
export function StreamingMessageRow({ name, seed, text }: { name: string; seed: string; text: string }) {
  return (
    <article aria-label={`${name} is writing`} className="aw-msg is-streaming">
      <div className="aw-msg__gutter">
        <Avatar kind="agent" name={name} seed={seed} size={36} working />
      </div>
      <div className="aw-msg__main">
        <div className="aw-msg__meta">
          <span className="aw-msg__author">{name}</span>
          <ActorTag kind="agent" />
          <Dots label="Writing" />
        </div>
        <div className="aw-msg__body">
          <Markdown className="aw-content" source={text} />
        </div>
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
