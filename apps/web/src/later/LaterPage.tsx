import { useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useSearchParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { Banner, Button, Dialog, EmptyState, ErrorState, ListSkeleton, PageHeader, SegmentedControl, TextArea, showToast } from '../ui'
import {
  cancelScheduled,
  changeSaved,
  invalidateLater,
  removeSaved,
  saveForLater,
  sendScheduledNow,
  updateScheduled,
  useLater,
  useScheduled,
  type SavedItem,
  type ScheduledMessage,
} from './api'
import { SavedRow, ScheduledRow } from './LaterRows'
import { TimeDialog } from './TimeDialog'
import { whenPhrase } from './when'
import './later.css'

type Tab = 'open' | 'done' | 'scheduled'

const TABS: { value: Tab; label: string }[] = [
  { value: 'open', label: 'Later' },
  { value: 'done', label: 'Done' },
  { value: 'scheduled', label: 'Scheduled' },
]

type Editing =
  | { kind: 'remind'; item: SavedItem }
  | { kind: 'reschedule'; item: ScheduledMessage }
  | { kind: 'edit'; item: ScheduledMessage }
  | { kind: 'new' }

function message(reason: unknown) {
  return isApiError(reason) ? reason.message : reason instanceof Error ? reason.message : 'Try again.'
}

/** Saved messages and tasks, reminders, and messages scheduled to send. */
export function LaterPage() {
  const session = useAuthenticatedSession()
  const queryClient = useQueryClient()
  const [params, setParams] = useSearchParams()
  const tab: Tab = TABS.some((option) => option.value === params.get('view')) ? (params.get('view') as Tab) : 'open'
  const saved = useLater(tab === 'done' ? 'done' : 'open')
  const scheduled = useScheduled(null, tab === 'scheduled')
  const [busy, setBusy] = useState<string | null>(null)
  const [editing, setEditing] = useState<Editing | null>(() => (params.get('new') === 'reminder' ? { kind: 'new' } : null))
  // Read once per render for "due" and relative days; the lists refetch each minute.
  const [now] = useState(() => new Date())

  const setTab = (next: Tab) => {
    const search = new URLSearchParams(params)
    if (next === 'open') search.delete('view')
    else search.set('view', next)
    search.delete('new')
    setParams(search, { replace: true })
  }

  const act = (id: string, work: () => Promise<unknown>, done?: string, undo?: () => Promise<unknown>) => {
    setBusy(id)
    void work()
      .then(() => {
        invalidateLater(queryClient)
        if (done) showToast({ tone: 'success', title: done, onUndo: undo ? () => undo().then(() => invalidateLater(queryClient)) : undefined })
      })
      .catch((reason: unknown) => showToast({ tone: 'danger', title: 'That didn’t work.', description: message(reason) }))
      .finally(() => setBusy(null))
  }

  const remind = (item: SavedItem, at: Date | null) =>
    act(item.id, () => changeSaved(item.id, { remindAt: at ? at.toISOString() : null }), at ? `I’ll remind you ${whenPhrase(at)}` : 'Reminder off')

  const closeNew = () => {
    setEditing(null)
    if (params.has('new')) {
      const search = new URLSearchParams(params)
      search.delete('new')
      setParams(search, { replace: true })
    }
  }

  const items = saved.data?.items ?? []
  const waiting = scheduled.data ?? []
  const loading = tab === 'scheduled' ? scheduled.isPending : saved.isPending
  const failed = tab === 'scheduled' ? scheduled.isError : saved.isError

  return (
    <AppShell home mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-later">
        <PageHeader
          actions={
            <Button icon="plus" onClick={() => setEditing({ kind: 'new' })}>
              New reminder
            </Button>
          }
          icon="bookmark"
          title="Later"
        >
          Messages and tasks you saved, your reminders, and messages waiting to send.
        </PageHeader>
        <div className="aw-later__tabs">
          <SegmentedControl label="Show" onChange={setTab} options={TABS} value={tab} />
          {tab === 'open' && saved.data ? (
            <span aria-live="polite" className="aw-later__count">
              {saved.data.openCount === 1 ? '1 item' : `${saved.data.openCount} items`}
            </span>
          ) : null}
        </div>

        {loading ? <ListSkeleton label="Loading" rows={5} /> : null}
        {failed ? (
          <ErrorState
            action={<Button icon="retry" onClick={() => void (tab === 'scheduled' ? scheduled.refetch() : saved.refetch())}>Try again</Button>}
            title="This list didn’t load"
          >
            Check your connection and try again.
          </ErrorState>
        ) : null}

        {tab !== 'scheduled' && saved.data ? (
          items.length ? (
            <ul aria-label={tab === 'done' ? 'Done' : 'Saved for later'} className="aw-later__list">
              {items.map((item) => (
                <SavedRow
                  busy={busy === item.id}
                  item={item}
                  key={item.id}
                  now={now}
                  onDone={(done) =>
                    act(item.id, () => changeSaved(item.id, { done }), done ? 'Marked done' : 'Moved back to Later', () => changeSaved(item.id, { done: !done }))
                  }
                  onRemind={(at) => remind(item, at)}
                  onRemindCustom={() => setEditing({ kind: 'remind', item })}
                  onRemove={() =>
                    act(item.id, () => removeSaved(item.id), 'Removed', () =>
                      saveForLater({
                        messageId: item.message?.id,
                        workItemId: item.workItem?.id,
                        note: item.note || undefined,
                        remindAt: item.remindAt,
                      }),
                    )
                  }
                />
              ))}
            </ul>
          ) : tab === 'done' ? (
            <EmptyState icon="circle-check" title="Nothing done yet">
              Items you mark done wait here in case you need them again.
            </EmptyState>
          ) : (
            <EmptyState
              action={<Button icon="plus" onClick={() => setEditing({ kind: 'new' })}>New reminder</Button>}
              icon="bookmark"
              title="Nothing saved for later"
            >
              Save a message with the bookmark on its toolbar, or type /remind in any conversation.
            </EmptyState>
          )
        ) : null}

        {tab === 'scheduled' && scheduled.data ? (
          waiting.length ? (
            <ul aria-label="Scheduled messages" className="aw-later__list">
              {waiting.map((item) => (
                <ScheduledRow
                  busy={busy === item.id}
                  item={item}
                  key={item.id}
                  now={now}
                  onCancel={() => act(item.id, () => cancelScheduled(item.id), item.status === 'failed' ? 'Dismissed' : 'Canceled')}
                  onEdit={() => setEditing({ kind: 'edit', item })}
                  onReschedule={() => setEditing({ kind: 'reschedule', item })}
                  onSendNow={() => act(item.id, () => sendScheduledNow(item.id), 'Sent')}
                />
              ))}
            </ul>
          ) : (
            <EmptyState icon="clock" title="Nothing scheduled">
              Use the clock next to Send, or type /schedule tomorrow 9am and your message.
            </EmptyState>
          )
        ) : null}
      </div>

      {editing?.kind === 'new' ? (
        <TimeDialog
          action="Set reminder"
          noteLabel="Remind me to"
          onClose={closeNew}
          onPick={async (at, note) => {
            await saveForLater({ note, remindAt: at.toISOString() })
            invalidateLater(queryClient)
            showToast({ tone: 'success', title: `I’ll remind you ${whenPhrase(at)}` })
          }}
          title="New reminder"
        />
      ) : null}
      {editing?.kind === 'remind' ? (
        <TimeDialog
          action="Remind me"
          initialAt={editing.item.remindAt ? new Date(editing.item.remindAt) : undefined}
          onClose={() => setEditing(null)}
          onPick={async (at) => {
            await changeSaved(editing.item.id, { remindAt: at.toISOString() })
            invalidateLater(queryClient)
          }}
          title="Remind me"
        />
      ) : null}
      {editing?.kind === 'reschedule' ? (
        <TimeDialog
          action="Reschedule"
          initialAt={new Date(editing.item.sendAt)}
          onClose={() => setEditing(null)}
          onPick={async (at) => {
            await updateScheduled(editing.item.id, { sendAt: at.toISOString() })
            invalidateLater(queryClient)
          }}
          title="Change when it sends"
        />
      ) : null}
      {editing?.kind === 'edit' ? <EditScheduledDialog item={editing.item} onClose={() => setEditing(null)} /> : null}
    </AppShell>
  )
}

function EditScheduledDialog({ item, onClose }: { item: ScheduledMessage; onClose: () => void }) {
  const queryClient = useQueryClient()
  const [text, setText] = useState(item.plainText)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!text.trim()) return
    setBusy(true)
    setError(null)
    try {
      await updateScheduled(item.id, { content: { version: 1, nodes: [{ type: 'text', text: text.trim() }] } })
      invalidateLater(queryClient)
      onClose()
    } catch (reason) {
      setError(message(reason))
    } finally {
      setBusy(false)
    }
  }
  return (
    <Dialog
      description={`Sends to ${item.place} ${whenPhrase(item.sendAt)}.`}
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={busy} disabled={!text.trim()} type="submit" variant="primary">
            Save
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => void submit(event)}
      open
      title="Edit scheduled message"
      width={520}
    >
      <TextArea data-autofocus label="Message" maxLength={20000} onChange={(event) => setText(event.target.value)} rows={4} value={text} />
      {error ? <Banner tone="danger">{error}</Banner> : null}
    </Dialog>
  )
}
