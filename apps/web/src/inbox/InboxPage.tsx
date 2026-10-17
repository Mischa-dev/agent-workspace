import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useSearchParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { formatFullDate } from '../conversations/timeFormat'
import { AppShell } from '../shell/AppShell'
import {
  Avatar,
  Banner,
  Button,
  EmptyState,
  ErrorState,
  Icon,
  Kbd,
  Menu,
  MessageSkeleton,
  PageHeader,
  SegmentedControl,
  StatusChip,
  type IconName,
} from '../ui'
import {
  groupBySubject,
  invalidateInbox,
  isExternalHref,
  respondToApproval,
  snoozeChoices,
  useInbox,
  useUpdateInbox,
  type InboxCategory,
  type InboxNotification,
  type InboxUpdate,
  type InboxView,
} from './api'
import { relativeTime } from './time'
import './inbox.css'

const VIEWS: { value: InboxView; label: string }[] = [
  { value: 'inbox', label: 'Inbox' },
  { value: 'unread', label: 'Unread' },
  { value: 'snoozed', label: 'Snoozed' },
  { value: 'archived', label: 'Archived' },
]

const CATEGORIES: { value: InboxCategory; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'mentions', label: 'Mentions' },
  { value: 'tasks', label: 'Tasks' },
  { value: 'runs', label: 'Runs' },
  { value: 'pull_requests', label: 'Pull requests' },
  { value: 'usage', label: 'Usage' },
]

const KIND_ICON: Record<string, IconName> = {
  mention: 'at-sign',
  comment_mention: 'at-sign',
  assigned: 'work',
  approval_requested: 'shield-check',
  run_waiting: 'clock',
  run_completed: 'circle-check',
  run_failed: 'circle-alert',
  checks_failed: 'triangle-alert',
  review_requested: 'pull-request',
  task_changed: 'work',
  capacity_reset_soon: 'clock',
  capacity_high_usage: 'activity',
  capacity_limit_reached: 'circle-alert',
  capacity_reset_credit: 'retry',
}

function isView(value: string | null): value is InboxView {
  return VIEWS.some((view) => view.value === value)
}

function isCategory(value: string | null): value is InboxCategory {
  return CATEGORIES.some((category) => category.value === value)
}

function typingTarget(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) return false
  return target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)
}

/** Everything that needs you, from every project: read, archive, snooze and act in place. */
export function InboxPage() {
  const session = useAuthenticatedSession()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [params, setParams] = useSearchParams()
  const view: InboxView = isView(params.get('view')) ? (params.get('view') as InboxView) : 'inbox'
  const category: InboxCategory = isCategory(params.get('category'))
    ? (params.get('category') as InboxCategory)
    : 'all'
  const inbox = useInbox(view, category)
  const mutation = useUpdateInbox(view, category)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [detailOpen, setDetailOpen] = useState(false)
  const [status, setStatus] = useState<string | null>(null)
  const [helpOpen, setHelpOpen] = useState(false)
  const [busyApproval, setBusyApproval] = useState<string | null>(null)
  const listRef = useRef<HTMLDivElement>(null)

  const notifications = useMemo(
    () => inbox.data?.pages.flatMap((page) => page.notifications) ?? [],
    [inbox.data],
  )
  const unreadCount = inbox.data?.pages[0]?.unreadCount ?? 0
  const groups = useMemo(() => groupBySubject(notifications), [notifications])
  // Keyboard order follows the grouped order on screen.
  const ordered = useMemo(() => groups.flatMap((group) => group.items), [groups])
  const selectedIndex = ordered.findIndex((item) => item.id === selectedId)
  const selected = selectedIndex >= 0 ? ordered[selectedIndex] : null

  const setFilter = (key: 'view' | 'category', value: string) => {
    const next = new URLSearchParams(params)
    if ((key === 'view' && value === 'inbox') || (key === 'category' && value === 'all')) next.delete(key)
    else next.set(key, value)
    setParams(next, { replace: true })
    setSelectedId(null)
    setDetailOpen(false)
  }

  const change = useCallback(
    (update: InboxUpdate, done?: string) => {
      setStatus(null)
      mutation.mutate(update, {
        onSuccess: () => {
          if (done) setStatus(done)
        },
        onError: (reason) =>
          setStatus(isApiError(reason) ? reason.message : 'That didn’t work. Try again.'),
      })
    },
    [mutation],
  )

  const select = useCallback(
    (item: InboxNotification | undefined, open = false) => {
      if (!item) return
      setSelectedId(item.id)
      if (open) setDetailOpen(true)
      if (!item.read && view !== 'archived') change({ action: 'read', ids: [item.id] })
      window.requestAnimationFrame(() => {
        listRef.current
          ?.querySelector<HTMLElement>(`[data-notification="${CSS.escape(item.id)}"]`)
          ?.scrollIntoView({ block: 'nearest' })
      })
    },
    [change, view],
  )

  /** After removing the selected row, move to the one that takes its place. */
  const advance = useCallback(() => {
    const next = ordered[selectedIndex + 1] ?? ordered[selectedIndex - 1]
    setSelectedId(next ? next.id : null)
    if (!next) setDetailOpen(false)
  }, [ordered, selectedIndex])

  const archive = useCallback(
    (item: InboxNotification) => {
      if (view === 'archived') change({ action: 'restore', ids: [item.id] }, 'Moved back to the inbox.')
      else change({ action: 'archive', ids: [item.id] }, 'Archived.')
      if (item.id === selectedId) advance()
    },
    [advance, change, selectedId, view],
  )

  const snooze = useCallback(
    (item: InboxNotification, until: Date, label: string) => {
      change({ action: 'snooze', ids: [item.id], snoozeUntil: until.toISOString() }, `Snoozed until ${label.toLowerCase()}.`)
      if (item.id === selectedId && view !== 'snoozed') advance()
    },
    [advance, change, selectedId, view],
  )

  const open = useCallback(
    (item: InboxNotification) => {
      if (!item.read) change({ action: 'read', ids: [item.id] })
      if (isExternalHref(item.href)) window.open(item.href, '_blank', 'noopener,noreferrer')
      else navigate(item.href)
    },
    [change, navigate],
  )

  const answer = async (item: InboxNotification, decision: 'approve' | 'deny') => {
    if (!item.approval) return
    setStatus(null)
    setBusyApproval(`${item.id}:${decision}`)
    try {
      await respondToApproval(item.approval.id, decision)
      setStatus(decision === 'approve' ? 'Approved.' : 'Denied.')
      invalidateInbox(queryClient)
    } catch (reason) {
      setStatus(isApiError(reason) ? reason.message : 'Couldn’t send your answer. Open the thread and try there.')
    } finally {
      setBusyApproval(null)
    }
  }

  const lastG = useRef(0)
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey || typingTarget(event.target)) return
      if (document.querySelector('[role="dialog"], [role="menu"]')) return
      const key = event.key
      // The second key of a global "G then …" shortcut (G S, G A) is not ours.
      const afterG = Date.now() - lastG.current <= 1_200
      lastG.current = key === 'g' ? Date.now() : 0
      if (afterG) return
      const current = selected ?? undefined
      if (key === 'j' || key === 'ArrowDown') {
        event.preventDefault()
        select(ordered[selectedIndex + 1] ?? ordered[selectedIndex] ?? ordered[0])
      } else if (key === 'k' || key === 'ArrowUp') {
        event.preventDefault()
        select(ordered[Math.max(selectedIndex - 1, 0)])
      } else if ((key === 'Enter' || key === 'o') && current) {
        event.preventDefault()
        open(current)
      } else if ((key === 'e' || key === 'y' || key === '#') && current) {
        event.preventDefault()
        archive(current)
      } else if (key === 'u' && current) {
        event.preventDefault()
        change({ action: current.read ? 'unread' : 'read', ids: [current.id] })
      } else if (key === 's' && current) {
        event.preventDefault()
        const tomorrow = snoozeChoices()[1]
        snooze(current, tomorrow.until, tomorrow.label)
      } else if (key === 'I' && event.shiftKey && view !== 'archived' && view !== 'snoozed') {
        event.preventDefault()
        change({ action: 'read', all: true }, 'Marked everything read.')
      } else if (key === 'a' && current?.approval?.status === 'pending' && current.approval.kind === 'approval') {
        event.preventDefault()
        void answer(current, 'approve')
      } else if (key === 'Escape' && detailOpen) {
        setDetailOpen(false)
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  })

  const caughtUp = !inbox.isPending && !inbox.isError && notifications.length === 0

  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-page--wide aw-inbox" data-detail-open={detailOpen && selected ? 'true' : 'false'}>
        <PageHeader
          actions={
            view === 'inbox' || view === 'unread' ? (
              <Button
                disabled={unreadCount === 0 || mutation.isPending}
                icon="check"
                onClick={() => change({ action: 'read', all: true }, 'Marked everything read.')}
              >
                Mark all read
              </Button>
            ) : null
          }
          icon="inbox"
          title="Inbox"
        >
          Mentions, assignments, approvals, runs and pull requests from every project. {unreadCount > 0 ? `${unreadCount} unread.` : null}
        </PageHeader>
        <div className="aw-inbox__filters">
          <SegmentedControl label="Show" onChange={(value) => setFilter('view', value)} options={VIEWS} value={view} />
          <div aria-label="Kind" className="aw-inbox__chips" role="group">
            {CATEGORIES.map((option) => (
              <button
                aria-pressed={option.value === category}
                className="aw-inbox__chip"
                key={option.value}
                onClick={() => setFilter('category', option.value)}
                type="button"
              >
                {option.label}
              </button>
            ))}
          </div>
          <button aria-expanded={helpOpen} className="aw-inbox__help-toggle" onClick={() => setHelpOpen((value) => !value)} type="button">
            Inbox shortcuts
          </button>
        </div>
        {helpOpen ? (
          <ul aria-label="Keyboard shortcuts" className="aw-inbox__help">
            <li><Kbd>j</Kbd> <Kbd>k</Kbd> next, previous</li>
            <li><Kbd>Enter</Kbd> open</li>
            <li><Kbd>e</Kbd> archive (restore in Archived)</li>
            <li><Kbd>u</Kbd> read or unread</li>
            <li><Kbd>s</Kbd> snooze until tomorrow</li>
            <li><Kbd>a</Kbd> approve</li>
            <li><Kbd>Shift</Kbd>+<Kbd>I</Kbd> mark all read</li>
          </ul>
        ) : null}
        <p aria-live="polite" className="aw-inbox__status" role="status">
          {status}
        </p>

        {inbox.isPending ? <MessageSkeleton label="Loading your inbox" rows={4} /> : null}
        {inbox.isError ? (
          <ErrorState action={<Button icon="retry" onClick={() => void inbox.refetch()}>Try again</Button>} title="Your inbox didn’t load">
            Check your connection and try again.
          </ErrorState>
        ) : null}
        {caughtUp ? (
          <EmptyState icon={view === 'archived' ? 'archive' : view === 'snoozed' ? 'clock' : 'circle-check'} title={emptyTitle(view)}>
            {emptyText(view, category)}
          </EmptyState>
        ) : null}

        {notifications.length > 0 ? (
          <div className="aw-inbox__panes">
            <div aria-label="Notifications" className="aw-inbox__list" ref={listRef} role="list">
              {groups.map((group) => (
                <section aria-label={group.title} className="aw-inbox__group" key={group.key}>
                  <h2 className="aw-inbox__group-title">
                    <span>{group.title}</span>
                    <span className="aw-inbox__group-project">{group.projectName}</span>
                  </h2>
                  {group.items.map((item) => (
                    <NotificationRow
                      item={item}
                      key={item.id}
                      onArchive={() => archive(item)}
                      onSelect={() => select(item, true)}
                      selected={item.id === selectedId}
                      view={view}
                    />
                  ))}
                </section>
              ))}
              {inbox.hasNextPage ? (
                <div className="aw-inbox__more">
                  <Button busy={inbox.isFetchingNextPage} onClick={() => void inbox.fetchNextPage()}>
                    Show older
                  </Button>
                </div>
              ) : null}
            </div>
            <aside aria-label="Preview" className="aw-inbox__detail">
              {selected ? (
                <NotificationDetail
                  busyApproval={busyApproval}
                  item={selected}
                  onAnswer={(decision) => void answer(selected, decision)}
                  onArchive={() => archive(selected)}
                  onBack={() => setDetailOpen(false)}
                  onOpen={() => open(selected)}
                  onSnooze={(until, label) => snooze(selected, until, label)}
                  onToggleRead={() => change({ action: selected.read ? 'unread' : 'read', ids: [selected.id] })}
                  onUnsnooze={() => change({ action: 'unsnooze', ids: [selected.id] }, 'Back in the inbox.')}
                  view={view}
                />
              ) : (
                <div className="aw-inbox__detail-empty">
                  <Icon name="inbox" size={24} />
                  <p>Pick a notification, or press <Kbd>j</Kbd> to start.</p>
                </div>
              )}
            </aside>
          </div>
        ) : null}
      </div>
    </AppShell>
  )
}

function emptyTitle(view: InboxView) {
  if (view === 'archived') return 'Nothing archived'
  if (view === 'snoozed') return 'Nothing snoozed'
  return 'You’re all caught up'
}

function emptyText(view: InboxView, category: InboxCategory) {
  if (category !== 'all') return 'Nothing of this kind here. Try All.'
  if (view === 'archived') return 'Archive notifications you’re done with and they’ll wait here.'
  if (view === 'snoozed') return 'Snooze a notification to bring it back later.'
  return 'When someone mentions you, assigns you a task, needs your approval or a run of yours ends, it shows up here.'
}

function NotificationRow({
  item,
  onArchive,
  onSelect,
  selected,
  view,
}: {
  item: InboxNotification
  onArchive: () => void
  onSelect: () => void
  selected: boolean
  view: InboxView
}) {
  return (
    <div
      aria-current={selected ? 'true' : undefined}
      className={`aw-inbox__row${item.read ? '' : ' is-unread'}${selected ? ' is-selected' : ''}`}
      data-notification={item.id}
      role="listitem"
    >
      <button className="aw-inbox__row-main" onClick={onSelect} type="button">
        <span aria-hidden="true" className="aw-inbox__dot" />
        {item.actor ? (
          <Avatar name={item.actor.name} seed={item.actor.id} size={28} />
        ) : (
          <span className="aw-inbox__kind-icon">
            <Icon name={KIND_ICON[item.kind] ?? 'info'} size={16} />
          </span>
        )}
        <span className="aw-inbox__row-text">
          <span className="aw-inbox__row-title">
            {item.read ? null : <span className="visually-hidden">Unread: </span>}
            {item.title}
            {item.count > 1 ? <span className="aw-inbox__count"> ×{item.count}</span> : null}
          </span>
          {item.preview ? <span className="aw-inbox__row-preview">{item.preview}</span> : null}
        </span>
        <time className="aw-inbox__time" dateTime={item.at} title={formatFullDate(item.at)}>
          {item.snoozedUntil ? `until ${relativeTime(item.snoozedUntil)}` : relativeTime(item.at)}
        </time>
      </button>
      <button
        aria-label={view === 'archived' ? `Restore: ${item.title}` : `Archive: ${item.title}`}
        className="aw-inbox__row-action"
        onClick={onArchive}
        type="button"
      >
        <Icon name={view === 'archived' ? 'undo' : 'archive'} size={16} />
      </button>
    </div>
  )
}

function NotificationDetail({
  busyApproval,
  item,
  onAnswer,
  onArchive,
  onBack,
  onOpen,
  onSnooze,
  onToggleRead,
  onUnsnooze,
  view,
}: {
  busyApproval: string | null
  item: InboxNotification
  onAnswer: (decision: 'approve' | 'deny') => void
  onArchive: () => void
  onBack: () => void
  onOpen: () => void
  onSnooze: (until: Date, label: string) => void
  onToggleRead: () => void
  onUnsnooze: () => void
  view: InboxView
}) {
  const pendingApproval = item.approval?.status === 'pending' && item.approval.kind === 'approval'
  const pendingQuestion = item.approval?.status === 'pending' && item.approval.kind === 'question'
  const external = isExternalHref(item.href)
  return (
    <article aria-labelledby="inbox-detail-title" className="aw-inbox__card">
      <button className="aw-inbox__back" onClick={onBack} type="button">
        <Icon name="arrow-left" size={16} /> Inbox
      </button>
      <p className="aw-inbox__card-meta">
        <StatusChip icon={KIND_ICON[item.kind] ?? 'info'}>{item.projectName}</StatusChip>
        <span>{item.subject.title}</span>
      </p>
      <h2 className="aw-inbox__card-title" id="inbox-detail-title">
        {item.title}
      </h2>
      <p className="aw-inbox__card-when">
        {formatFullDate(item.at)}
        {item.count > 1 ? ` · ${item.count} times` : ''}
        {item.snoozedUntil ? ` · snoozed until ${formatFullDate(item.snoozedUntil)}` : ''}
      </p>
      {item.preview ? <blockquote className="aw-inbox__quote">{item.preview}</blockquote> : null}
      {item.approval && item.approval.status !== 'pending' ? (
        <Banner tone="info">This was already answered.</Banner>
      ) : null}
      <div className="aw-inbox__actions">
        {pendingApproval ? (
          <>
            <Button
              busy={busyApproval === `${item.id}:approve`}
              disabled={busyApproval !== null}
              icon="check"
              onClick={() => onAnswer('approve')}
              variant="primary"
            >
              Approve
            </Button>
            <Button
              busy={busyApproval === `${item.id}:deny`}
              disabled={busyApproval !== null}
              icon="x"
              onClick={() => onAnswer('deny')}
              variant="danger"
            >
              Deny
            </Button>
          </>
        ) : null}
        {external ? (
          <Button icon="external" onClick={onOpen} variant={pendingApproval ? 'secondary' : 'primary'}>
            Open on GitHub
          </Button>
        ) : (
          <Link className={`aw-btn aw-btn--${pendingApproval ? 'secondary' : 'primary'} aw-btn--md`} to={item.href}>
            <Icon name="arrow-up" size={16} />
            <span className="aw-btn__label">{pendingQuestion ? 'Answer in the thread' : 'Open'}</span>
          </Link>
        )}
        {item.launchHref ? (
          <Link className="aw-btn aw-btn--secondary aw-btn--md" to={item.launchHref}>
            <Icon name="play" size={16} />
            <span className="aw-btn__label">Launch</span>
          </Link>
        ) : null}
        <Button icon={view === 'archived' ? 'undo' : 'archive'} onClick={onArchive}>
          {view === 'archived' ? 'Restore' : 'Archive'}
        </Button>
        {view === 'snoozed' ? (
          <Button icon="undo" onClick={onUnsnooze}>
            Unsnooze
          </Button>
        ) : view !== 'archived' ? (
          <Menu
            items={snoozeChoices().map((choice) => ({
              label: `${choice.label} (${formatFullDate(choice.until.toISOString())})`,
              icon: 'clock' as const,
              onSelect: () => onSnooze(choice.until, choice.label),
            }))}
            label="Snooze until"
            trigger={
              <Button icon="clock" iconEnd="chevron-down">
                Snooze
              </Button>
            }
          />
        ) : null}
        <Button onClick={onToggleRead} variant="ghost">
          {item.read ? 'Mark unread' : 'Mark read'}
        </Button>
      </div>
    </article>
  )
}
