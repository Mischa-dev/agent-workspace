import { useState, type FormEvent } from 'react'

import {
  Banner,
  Button,
  Dialog,
  EmptyState,
  IconButton,
  ListSkeleton,
  SegmentedControl,
  SelectField,
  StatusChip,
  TextField,
  countLabel,
  describeError,
  formatFullDate,
  formatRelative,
  type ChipTone,
} from '../ui'
import { updateThread } from './api'
import { THREAD_STATUSES, isSettled, useThreads } from './threadModel'
import type { ThreadSummary } from './types'
import '../styles/conversation.css'

function statusTone(status: string): ChipTone {
  switch (status.toLowerCase()) {
    case 'decided':
    case 'done':
      return 'success'
    case 'blocked':
      return 'warning'
    case 'in progress':
      return 'agent'
    default:
      return 'muted'
  }
}

export function ThreadStatusChip({ status }: { status: string }) {
  return <StatusChip tone={statusTone(status)}>{status}</StatusChip>
}

/** Rename a thread, or set its status and a one-line outcome. */
export function ThreadStatusDialog({
  onClose,
  onSaved,
  thread,
}: {
  onClose: () => void
  onSaved: () => void
  thread: { id: string; name: string; status: string; outcome: string | null }
}) {
  const known = THREAD_STATUSES.some((status) => status.toLowerCase() === thread.status.toLowerCase())
  const [name, setName] = useState(thread.name)
  const [choice, setChoice] = useState(known ? thread.status : 'custom')
  const [custom, setCustom] = useState(known ? '' : thread.status)
  const [outcome, setOutcome] = useState(thread.outcome ?? '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const status = choice === 'custom' ? custom.trim() : choice

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim() || !status || busy) return
    setBusy(true)
    setError(null)
    const trimmedOutcome = outcome.trim()
    void updateThread(thread.id, {
      ...(name.trim() !== thread.name ? { name: name.trim() } : {}),
      status,
      ...(trimmedOutcome ? { outcome: trimmedOutcome } : thread.outcome ? { clearOutcome: true } : {}),
    })
      .then(() => {
        onSaved()
        onClose()
      })
      .catch((reason: unknown) => setError(`The thread status wasn’t saved. ${describeError(reason).message}`))
      .finally(() => setBusy(false))
  }

  return (
    <Dialog
      description="Say where this thread stands, so nobody has to reread it to find out."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} disabled={!name.trim() || !status} type="submit" variant="primary">
            Save
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title="Thread status"
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
      <TextField label="Name" maxLength={120} onChange={(event) => setName(event.target.value)} value={name} />
      <SelectField label="Status" onChange={(event) => setChoice(event.target.value)} value={choice}>
        {THREAD_STATUSES.map((option) => (
          <option key={option} value={option}>
            {option}
          </option>
        ))}
        <option value="custom">Something else…</option>
      </SelectField>
      {choice === 'custom' ? (
        <TextField autoFocus label="Status name" maxLength={40} onChange={(event) => setCustom(event.target.value)} value={custom} />
      ) : null}
      <TextField
        hint="One line on where it landed, such as “Going with design tokens; AW-51 created”."
        label="Outcome"
        maxLength={1000}
        onChange={(event) => setOutcome(event.target.value)}
        value={outcome}
      />
    </Dialog>
  )
}

/** A button showing a thread's status that opens the status dialog. */
export function ThreadStatusButton({
  onSaved,
  quietWhenOpen = false,
  thread,
}: {
  onSaved: () => void
  /** An open thread shows a small "Set status" button instead of an "Open" chip that says nothing. */
  quietWhenOpen?: boolean
  thread: { id: string; name: string; status: string; outcome: string | null }
}) {
  const [open, setOpen] = useState(false)
  return (
    <>
      {quietWhenOpen && thread.status === 'Open' ? (
        <IconButton icon="circle-check" label="Set status" onClick={() => setOpen(true)} size="sm" />
      ) : (
        <button
          aria-label={`Status: ${thread.status}. Change it`}
          className="aw-threadstatus"
          onClick={() => setOpen(true)}
          title={thread.outcome ?? 'Set the status and outcome'}
          type="button"
        >
          <ThreadStatusChip status={thread.status} />
        </button>
      )}
      {open ? <ThreadStatusDialog onClose={() => setOpen(false)} onSaved={onSaved} thread={thread} /> : null}
    </>
  )
}

type Filter = 'open' | 'settled' | 'all'

/** Every thread under a conversation with its status and outcome: the conversation's board. */
export function ThreadBoard({
  conversationId,
  onClose,
  onOpen,
  onStart,
  projectId,
}: {
  conversationId: string
  projectId: string
  onClose: () => void
  onOpen: (thread: ThreadSummary) => void
  /** Offers "Start a thread" when there are none; leave it out where you can't post. */
  onStart?: () => void
}) {
  const threads = useThreads(projectId, conversationId)
  const [filter, setFilter] = useState<Filter>('open')
  const shown = (threads.data ?? []).filter((thread) =>
    filter === 'all' ? true : filter === 'open' ? !isSettled(thread.status) : isSettled(thread.status),
  )
  return (
    <aside aria-label="Threads" className="context-drawer aw-threadboard">
      <div className="aw-members__head">
        <h2>Threads</h2>
        <IconButton className="aw-panelclose" icon="x" label="Close threads" onClick={onClose} size="sm" />
      </div>
      <SegmentedControl<Filter>
        label="Show threads"
        onChange={setFilter}
        options={[
          { value: 'open', label: 'Open' },
          { value: 'settled', label: 'Settled' },
          { value: 'all', label: 'All' },
        ]}
        value={filter}
      />
      {threads.isPending ? <ListSkeleton label="Loading threads" rows={3} /> : null}
      {threads.isError ? (
        <Banner onRetry={() => void threads.refetch()} title="Threads didn’t load." tone="danger">
          {describeError(threads.error).message}
        </Banner>
      ) : null}
      {threads.isSuccess && shown.length === 0 ? (
        <EmptyState
          action={
            onStart && filter !== 'settled' ? (
              <Button icon="messages" onClick={onStart} size="sm">
                Start a thread
              </Button>
            ) : undefined
          }
          icon="messages"
          title={filter === 'settled' ? 'Nothing settled yet' : 'No threads here'}
        >
          {filter === 'settled'
            ? 'Threads marked Decided, Done or Dropped show up here.'
            : 'Start a thread from any message when a topic is going to run long.'}
        </EmptyState>
      ) : null}
      <ul className="aw-threadboard__list">
        {shown.map((thread) => (
          <li key={thread.id}>
            <button className="aw-threadboard__item" onClick={() => onOpen(thread)} type="button">
              <span className="aw-threadboard__top">
                <span className="aw-threadboard__name">{thread.name}</span>
                {thread.unread ? <span aria-label="New replies" className="aw-badge aw-badge--mention aw-badge--dot" role="img" /> : null}
              </span>
              <span className="aw-threadboard__meta">
                {/* "Open" says nothing here: the filter already does. */}
                {thread.status === 'Open' ? null : <ThreadStatusChip status={thread.status} />}
                <span>{thread.messageCount === 0 ? 'No replies yet' : countLabel(thread.messageCount, 'reply', 'replies')}</span>
                {thread.lastActivityAt ? (
                  <time dateTime={thread.lastActivityAt} title={formatFullDate(thread.lastActivityAt)}>
                    · {formatRelative(thread.lastActivityAt)}
                  </time>
                ) : null}
              </span>
              {thread.outcome ? <span className="aw-threadboard__outcome">{thread.outcome}</span> : null}
            </button>
          </li>
        ))}
      </ul>
    </aside>
  )
}
