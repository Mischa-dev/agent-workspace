import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate, useSearchParams } from 'react-router-dom'

import { toastError } from '../app/mutations'
import { useAuthenticatedSession } from '../auth/session'
import { atPhrase } from '../later/when'
import { AppShell } from '../shell/AppShell'
import {
  Avatar,
  Button,
  EmptyState,
  ErrorState,
  Icon,
  IconButton,
  Kbd,
  ListSkeleton,
  PageHeader,
  SegmentedControl,
  countLabel,
  formatFullDate,
  formatRelativeShort,
  formatShortDate,
  formatTime,
  plainPreview,
  showToast,
  type ToastInput,
} from '../ui'
import {
  groupBySubject,
  invalidateInbox,
  isExternalHref,
  postUpdate,
  respondToApproval,
  snoozeChoices,
  unreadIds,
  useInbox,
  useUpdateInbox,
  type InboxCategory,
  type InboxNotification,
  type InboxUpdate,
  type InboxUpdateResult,
  type InboxView,
} from './api'
import { FAILURE_KINDS, KIND_ICON } from './kinds'
import { NotificationDetail } from './NotificationDetail'
import './inbox.css'

const VIEWS: { value: InboxView; label: string }[] = [
  { value: 'inbox', label: 'Inbox' },
  { value: 'unread', label: 'Unread' },
  { value: 'snoozed', label: 'Snoozed' },
  { value: 'archived', label: 'Archived' },
]

/** Each filter, and what "mark all read" marks under it. */
const CATEGORIES: { value: InboxCategory; label: string; markRead: string }[] = [
  { value: 'all', label: 'All', markRead: 'Mark all read' },
  { value: 'mentions', label: 'Mentions', markRead: 'Mark mentions read' },
  { value: 'tasks', label: 'Tasks', markRead: 'Mark task updates read' },
  { value: 'runs', label: 'Runs', markRead: 'Mark run updates read' },
  { value: 'pull_requests', label: 'Pull requests', markRead: 'Mark pull request updates read' },
  { value: 'usage', label: 'Usage', markRead: 'Mark usage alerts read' },
]

const PHONE = '(max-width: 860px)'

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

/** When a snoozed row comes back, short enough for the time column. */
function backAt(iso: string, now = new Date()) {
  const days = Math.round((new Date(iso).setHours(0, 0, 0, 0) - new Date(now).setHours(0, 0, 0, 0)) / 86_400_000)
  if (days <= 0) return formatTime(iso)
  if (days === 1) return 'tomorrow'
  return formatShortDate(iso, now)
}

type ChangeOptions = {
  /** What to say when it worked, usually with Undo. */
  success?: (result: InboxUpdateResult) => ToastInput | null
  /** The toast title when it fails. */
  failed: string
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
  const filter = CATEGORIES.find((option) => option.value === category) ?? CATEGORIES[0]
  const inbox = useInbox(view, category)
  const mutation = useUpdateInbox(view, category)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [detailOpen, setDetailOpen] = useState(false)
  const [helpOpen, setHelpOpen] = useState(false)
  const [busyApproval, setBusyApproval] = useState<string | null>(null)
  const [marking, setMarking] = useState(false)
  // In Unread, rows you read stay put until you leave the view, so you can still act on them.
  const [kept, setKept] = useState<Map<string, InboxNotification>>(() => new Map())
  const listRef = useRef<HTMLDivElement>(null)
  const pageRef = useRef<HTMLDivElement>(null)
  const listScroll = useRef(0)

  const loaded = useMemo(() => inbox.data?.pages.flatMap((page) => page.notifications) ?? [], [inbox.data])
  const notifications = useMemo(() => {
    if (view !== 'unread' || kept.size === 0) return loaded
    const present = new Set(loaded.map((item) => item.id))
    const extra = [...kept.values()].filter((item) => !present.has(item.id))
    if (extra.length === 0) return loaded
    return [...loaded, ...extra].sort((a, b) => (a.at < b.at ? 1 : a.at > b.at ? -1 : 0))
  }, [kept, loaded, view])
  const unreadCount = inbox.data?.pages[0]?.unreadCount ?? 0
  const groups = useMemo(() => groupBySubject(notifications), [notifications])
  // Keyboard order follows the grouped order on screen.
  const ordered = useMemo(() => groups.flatMap((group) => group.items), [groups])
  const selectedIndex = ordered.findIndex((item) => item.id === selectedId)
  const selected = selectedIndex >= 0 ? ordered[selectedIndex] : null
  const canMarkAll =
    view !== 'archived' && view !== 'snoozed' && (category === 'all' ? unreadCount > 0 : notifications.some((item) => !item.read) || inbox.hasNextPage)

  const keep = useCallback(
    (item: InboxNotification, read: boolean | null) => {
      if (view !== 'unread') return
      setKept((current) => {
        const next = new Map(current)
        if (read === null) next.delete(item.id)
        else next.set(item.id, { ...item, read })
        return next
      })
    },
    [view],
  )

  // On a phone the preview replaces the list: start it at the top, and come back to the same place in the list.
  const showingDetail = detailOpen && Boolean(selected)
  useLayoutEffect(() => {
    const scroller = pageRef.current?.closest('main')
    if (!scroller || !window.matchMedia(PHONE).matches) return
    if (showingDetail) scroller.scrollTop = 0
    else scroller.scrollTop = listScroll.current
  }, [showingDetail])

  const setFilter = (key: 'view' | 'category', value: string) => {
    const next = new URLSearchParams(params)
    if ((key === 'view' && value === 'inbox') || (key === 'category' && value === 'all')) next.delete(key)
    else next.set(key, value)
    setParams(next, { replace: true })
    setSelectedId(null)
    setDetailOpen(false)
    setKept(new Map())
  }

  const undo = useCallback(
    (update: InboxUpdate) => postUpdate(update).then(() => invalidateInbox(queryClient)),
    [queryClient],
  )

  const change = useCallback(
    (update: InboxUpdate, options: ChangeOptions) => {
      mutation.mutate(update, {
        onSuccess: (result) => {
          const toast = options.success?.(result)
          if (toast) showToast(toast)
        },
        onError: (reason) => toastError(options.failed, reason),
      })
    },
    [mutation],
  )

  const setRead = useCallback(
    (item: InboxNotification, read: boolean) => {
      keep(item, read)
      change({ action: read ? 'read' : 'unread', ids: [item.id] }, { failed: read ? 'It wasn’t marked read' : 'It wasn’t marked unread' })
    },
    [change, keep],
  )

  const select = useCallback(
    (item: InboxNotification | undefined, open = false) => {
      if (!item) return
      if (open && !detailOpen) listScroll.current = pageRef.current?.closest('main')?.scrollTop ?? 0
      setSelectedId(item.id)
      if (open) setDetailOpen(true)
      if (!item.read && view !== 'archived') setRead(item, true)
      // Focus moves along with the selection when it was in the list already.
      const focusRow = !open && Boolean(listRef.current?.contains(document.activeElement))
      window.requestAnimationFrame(() => {
        const row = listRef.current?.querySelector<HTMLElement>(`[data-notification="${CSS.escape(item.id)}"]`)
        row?.scrollIntoView({ block: 'nearest' })
        if (focusRow) row?.querySelector<HTMLElement>('.aw-inbox__row-main')?.focus({ preventScroll: true })
      })
    },
    [detailOpen, setDetailOpen, setRead, view],
  )

  /** After removing the selected row, move to the one that takes its place. */
  const advance = useCallback(() => {
    const next = ordered[selectedIndex + 1] ?? ordered[selectedIndex - 1]
    setSelectedId(next ? next.id : null)
    if (!next) setDetailOpen(false)
  }, [ordered, selectedIndex, setDetailOpen])

  const archive = useCallback(
    (item: InboxNotification) => {
      const ids = [item.id]
      keep(item, null)
      if (view === 'archived') {
        change(
          { action: 'restore', ids },
          {
            success: () => ({ tone: 'success', title: 'Moved back to the inbox', onUndo: () => undo({ action: 'archive', ids }) }),
            failed: 'It wasn’t moved back to the inbox',
          },
        )
      } else {
        change(
          { action: 'archive', ids },
          {
            success: () => ({ tone: 'success', title: 'Archived', onUndo: () => undo({ action: 'restore', ids }) }),
            failed: 'It wasn’t archived',
          },
        )
      }
      if (item.id === selectedId) advance()
    },
    [advance, change, keep, selectedId, undo, view],
  )

  const snooze = useCallback(
    (item: InboxNotification, until: Date) => {
      const ids = [item.id]
      keep(item, null)
      change(
        { action: 'snooze', ids, snoozeUntil: until.toISOString() },
        {
          success: () => ({ tone: 'success', title: `Snoozed until ${atPhrase(until)}`, onUndo: () => undo({ action: 'unsnooze', ids }) }),
          failed: 'It wasn’t snoozed',
        },
      )
      if (item.id === selectedId && view !== 'snoozed') advance()
    },
    [advance, change, keep, selectedId, undo, view],
  )

  const unsnooze = (item: InboxNotification) => {
    change({ action: 'unsnooze', ids: [item.id] }, { success: () => ({ tone: 'success', title: 'Back in the inbox' }), failed: 'It’s still snoozed' })
    if (item.id === selectedId) advance()
  }

  const open = useCallback(
    (item: InboxNotification) => {
      if (!item.read) setRead(item, true)
      if (isExternalHref(item.href)) window.open(item.href, '_blank', 'noopener,noreferrer')
      else navigate(item.href)
    },
    [navigate, setRead],
  )

  const markedRead = (result: InboxUpdateResult): ToastInput | null =>
    result.updated.length
      ? {
          tone: 'success',
          title: `Marked ${countLabel(result.updated.length, 'notification')} read`,
          onUndo: () => undo({ action: 'unread', ids: result.updated }),
        }
      : null

  /** Marks what the current filter shows read: everything, or one kind. */
  const markAllRead = async () => {
    setKept(new Map())
    if (category === 'all') {
      change({ action: 'read', all: true }, { success: markedRead, failed: 'Nothing was marked read' })
      return
    }
    setMarking(true)
    try {
      const ids = await unreadIds(category)
      if (ids.length) change({ action: 'read', ids }, { success: markedRead, failed: 'Nothing was marked read' })
      else showToast({ title: 'Nothing here is unread' })
    } catch (reason) {
      toastError('Nothing was marked read', reason)
    } finally {
      setMarking(false)
    }
  }

  const answer = async (item: InboxNotification, decision: 'approve' | 'deny') => {
    if (!item.approval) return
    setBusyApproval(`${item.id}:${decision}`)
    try {
      await respondToApproval(item.approval.id, decision)
      showToast({ tone: 'success', title: decision === 'approve' ? 'Approved' : 'Denied' })
      invalidateInbox(queryClient)
    } catch (reason) {
      toastError('Your answer wasn’t sent', reason)
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
      } else if (key === 'Home' || key === 'End') {
        event.preventDefault()
        select(key === 'Home' ? ordered[0] : ordered[ordered.length - 1])
      } else if ((key === 'Enter' || key === 'o') && current) {
        event.preventDefault()
        open(current)
      } else if ((key === 'e' || key === 'y' || key === '#') && current) {
        event.preventDefault()
        archive(current)
      } else if (key === 'u' && current) {
        event.preventDefault()
        setRead(current, !current.read)
      } else if (key === 's' && current && view !== 'snoozed' && view !== 'archived') {
        event.preventDefault()
        const tomorrow = snoozeChoices().find((choice) => choice.id === 'tomorrow')
        if (tomorrow) snooze(current, tomorrow.until)
      } else if (key === 'I' && event.shiftKey && canMarkAll && !marking) {
        event.preventDefault()
        void markAllRead()
      } else if (key === 'a' && current?.approval?.status === 'pending' && current.approval.kind === 'approval') {
        event.preventDefault()
        void answer(current, 'approve')
      } else if (key === 'Escape' && detailOpen) {
        setDetailOpen(false)
      } else if (key === 'Escape' && selected) {
        // Beside the list, the preview closes back to its empty state.
        setSelectedId(null)
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  })

  const caughtUp = !inbox.isPending && !inbox.isError && notifications.length === 0

  return (
    <AppShell home mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-inbox" data-detail-open={showingDetail ? 'true' : 'false'} ref={pageRef}>
        <PageHeader
          actions={
            view === 'inbox' || view === 'unread' ? (
              <Button busy={marking} disabled={!canMarkAll || mutation.isPending} icon="check" onClick={() => void markAllRead()}>
                {filter.markRead}
              </Button>
            ) : null
          }
          icon="inbox"
          title="Inbox"
        >
          Mentions, assignments, approvals, runs and pull requests from every project.{unreadCount > 0 ? ` ${unreadCount} unread.` : null}
        </PageHeader>
        <div className="aw-inbox__filters">
          <SegmentedControl label="Show" onChange={(value) => setFilter('view', value)} options={VIEWS} value={view} />
          <div aria-label="Kind" className="aw-inbox__chips" role="group">
            {CATEGORIES.map((option) => (
              <Button
                aria-pressed={option.value === category}
                className="aw-inbox__chip"
                key={option.value}
                onClick={() => setFilter('category', option.value)}
                size="sm"
                variant="ghost"
              >
                {option.label}
              </Button>
            ))}
          </div>
          <Button aria-expanded={helpOpen} className="aw-inbox__help-toggle" onClick={() => setHelpOpen((value) => !value)} size="sm" variant="ghost">
            Shortcuts
          </Button>
        </div>
        {helpOpen ? (
          <ul aria-label="Keyboard shortcuts" className="aw-inbox__help">
            <li><Kbd>j</Kbd> <Kbd>k</Kbd> or <Kbd>↓</Kbd> <Kbd>↑</Kbd> next, previous</li>
            <li><Kbd>Home</Kbd> <Kbd>End</Kbd> first, last</li>
            <li><Kbd>Enter</Kbd> open</li>
            <li><Kbd>Esc</Kbd> close the preview</li>
            <li><Kbd>e</Kbd> archive (restore in Archived)</li>
            <li><Kbd>u</Kbd> read or unread</li>
            <li><Kbd>s</Kbd> snooze until tomorrow</li>
            <li><Kbd>a</Kbd> approve</li>
            <li><Kbd>Shift</Kbd>+<Kbd>I</Kbd> {filter.markRead.toLowerCase()}</li>
          </ul>
        ) : null}

        {inbox.isPending ? <ListSkeleton avatars label="Loading your inbox" rows={4} /> : null}
        {inbox.isError ? <ErrorState error={inbox.error} onRetry={() => void inbox.refetch()} title="Your inbox didn’t load" /> : null}
        {caughtUp ? (
          <EmptyState
            action={
              category !== 'all' ? (
                <Button onClick={() => setFilter('category', 'all')}>Show all kinds</Button>
              ) : view !== 'inbox' ? (
                <Button onClick={() => setFilter('view', 'inbox')}>Back to the inbox</Button>
              ) : null
            }
            icon={view === 'archived' ? 'archive' : view === 'snoozed' ? 'clock' : 'circle-check'}
            title={emptyTitle(view)}
          >
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
                  onSnooze={(until) => snooze(selected, until)}
                  onToggleRead={() => setRead(selected, !selected.read)}
                  onUnsnooze={() => unsnooze(selected)}
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
  if (view === 'unread') return 'Nothing unread'
  return 'You’re all caught up'
}

function emptyText(view: InboxView, category: InboxCategory) {
  if (category !== 'all') return 'Nothing of this kind here.'
  if (view === 'archived') return 'Archive notifications you’re done with and they’ll wait here.'
  if (view === 'snoozed') return 'Snooze a notification to bring it back later.'
  if (view === 'unread') return 'You’ve read everything in your inbox.'
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
  const preview = plainPreview(item.preview, 240)
  const kindTone = FAILURE_KINDS.has(item.kind) ? ' is-failure' : ''
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
          <Avatar kind={item.actor.kind} name={item.actor.name} seed={item.actor.id} size={28} />
        ) : (
          <span className={`aw-inbox__kind-icon${kindTone}`}>
            <Icon name={KIND_ICON[item.kind] ?? 'info'} size={16} />
          </span>
        )}
        <span className="aw-inbox__row-text">
          <span className="aw-inbox__row-title">
            {item.read ? null : <span className="visually-hidden">Unread: </span>}
            {item.title}
            {item.count > 1 ? <span className="aw-inbox__count"> ×{item.count}</span> : null}
          </span>
          {preview ? <span className="aw-inbox__row-preview">{preview}</span> : null}
        </span>
        {item.snoozedUntil ? (
          <time className="aw-inbox__time" dateTime={item.snoozedUntil} title={`Back ${formatFullDate(item.snoozedUntil)}`}>
            <Icon name="clock" size={12} /> {backAt(item.snoozedUntil)}
          </time>
        ) : (
          <time className="aw-inbox__time" dateTime={item.at} title={formatFullDate(item.at)}>
            {formatRelativeShort(item.at)}
          </time>
        )}
      </button>
      <IconButton
        className="aw-inbox__row-action"
        icon={view === 'archived' ? 'undo' : 'archive'}
        label={view === 'archived' ? `Restore: ${item.title}` : `Archive: ${item.title}`}
        onClick={onArchive}
        size="sm"
        tooltip={false}
      />
    </div>
  )
}
