import { useEffect, useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useSearchParams } from 'react-router-dom'

import { toastError } from '../app/mutations'
import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import {
  Banner,
  Button,
  ConfirmDialog,
  Dialog,
  EmptyState,
  ErrorState,
  ListSkeleton,
  PageHeader,
  SegmentedControl,
  TextArea,
  countLabel,
  describeError,
  plainPreview,
  showToast,
} from '../ui'
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
import { atPhrase, whenPhrase } from './when'
import './later.css'

type Tab = 'open' | 'done' | 'scheduled'

const TABS: { value: Tab; label: string }[] = [
  { value: 'open', label: 'Saved' },
  { value: 'done', label: 'Done' },
  { value: 'scheduled', label: 'Scheduled' },
]

type Editing =
  | { kind: 'remind'; item: SavedItem }
  | { kind: 'reschedule'; item: ScheduledMessage }
  | { kind: 'edit'; item: ScheduledMessage }
  | { kind: 'new' }

/** "Due" chips, relative days and the quick times all move on with the clock. */
function useMinuteClock() {
  const [now, setNow] = useState(() => new Date())
  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), 60_000)
    return () => window.clearInterval(timer)
  }, [])
  return now
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
  const [discarding, setDiscarding] = useState<ScheduledMessage | null>(null)
  const now = useMinuteClock()

  const setTab = (next: Tab) => {
    const search = new URLSearchParams(params)
    if (next === 'open') search.delete('view')
    else search.set('view', next)
    search.delete('new')
    setParams(search, { replace: true })
  }

  /** One change to a row: busy while it runs, a toast after, and Undo where it can be undone. */
  const act = (
    id: string,
    work: () => Promise<unknown>,
    messages: { done?: string; failed: string },
    undo?: () => Promise<unknown>,
  ) => {
    setBusy(id)
    return work()
      .then(() => {
        invalidateLater(queryClient)
        if (messages.done) {
          showToast({
            tone: 'success',
            title: messages.done,
            onUndo: undo ? () => undo().then(() => invalidateLater(queryClient)).catch((reason: unknown) => toastError('It wasn’t undone', reason)) : undefined,
          })
        }
      })
      .catch((reason: unknown) => {
        toastError(messages.failed, reason)
      })
      .finally(() => setBusy(null))
  }

  const remind = (item: SavedItem, at: Date | null) =>
    void act(
      item.id,
      () => changeSaved(item.id, { remindAt: at ? at.toISOString() : null }),
      { done: at ? `Reminder set for ${atPhrase(at)}` : 'Reminder off', failed: at ? 'The reminder wasn’t set' : 'The reminder wasn’t turned off' },
      () => changeSaved(item.id, { remindAt: item.remindAt && new Date(item.remindAt).getTime() > Date.now() ? item.remindAt : null }),
    )

  const remove = (item: SavedItem) =>
    void act(item.id, () => removeSaved(item.id), { done: 'Removed from Later', failed: 'It wasn’t removed' }, () =>
      saveForLater({
        messageId: item.message?.id,
        workItemId: item.workItem?.id,
        note: item.note || undefined,
        // A reminder that already went off comes back without a time, rather than failing for being in the past.
        remindAt: item.remindAt && new Date(item.remindAt).getTime() > Date.now() ? item.remindAt : null,
      }),
    )

  const copyText = (item: ScheduledMessage) =>
    void navigator.clipboard
      ?.writeText(item.plainText)
      .then(() => showToast({ tone: 'success', title: 'Text copied' }))
      .catch((reason: unknown) => toastError('The text wasn’t copied', reason))

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
  const list = tab === 'scheduled' ? scheduled : saved

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
          title="Later" />
        <div className="aw-later__tabs">
          <SegmentedControl label="Show" onChange={setTab} options={TABS} value={tab} />
          {tab === 'open' && saved.data ? (
            <span aria-live="polite" className="aw-later__count">
              {countLabel(saved.data.openCount, 'item')}
            </span>
          ) : null}
        </div>

        {list.isPending ? <ListSkeleton label="Loading" rows={5} /> : null}
        {list.isError ? <ErrorState error={list.error} onRetry={() => void list.refetch()} title="This list didn’t load" /> : null}

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
                    void act(
                      item.id,
                      () => changeSaved(item.id, { done }),
                      { done: done ? 'Marked done' : 'Moved back to Saved', failed: done ? 'It wasn’t marked done' : 'It wasn’t moved back' },
                      () => changeSaved(item.id, { done: !done }),
                    )
                  }
                  onRemind={(at) => remind(item, at)}
                  onRemindCustom={() => setEditing({ kind: 'remind', item })}
                  onRemove={() => remove(item)}
                />
              ))}
            </ul>
          ) : tab === 'done' ? (
            <EmptyState action={<Button onClick={() => setTab('open')}>See saved items</Button>} icon="circle-check" title="Nothing done yet">
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
                  onCancel={() => setDiscarding(item)}
                  onCopy={() => copyText(item)}
                  onEdit={() => setEditing({ kind: 'edit', item })}
                  onReschedule={() => setEditing({ kind: 'reschedule', item })}
                  onSendNow={() =>
                    void act(item.id, () => sendScheduledNow(item.id), {
                      done: 'Sent',
                      failed: item.status === 'failed' ? 'It still didn’t send' : 'It wasn’t sent',
                    })
                  }
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
            showToast({ tone: 'success', title: `Reminder set for ${atPhrase(at)}`, description: note })
          }}
          title="New reminder"
        />
      ) : null}
      {editing?.kind === 'remind' ? (
        <TimeDialog
          action="Set reminder"
          initialAt={editing.item.remindAt ? new Date(editing.item.remindAt) : undefined}
          onClose={() => setEditing(null)}
          onPick={async (at) => {
            await changeSaved(editing.item.id, { remindAt: at.toISOString() })
            invalidateLater(queryClient)
            showToast({ tone: 'success', title: `Reminder set for ${atPhrase(at)}` })
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
            showToast({ tone: 'success', title: `Sends ${whenPhrase(at)}` })
          }}
          title="Change when it sends"
        />
      ) : null}
      {editing?.kind === 'edit' ? <EditScheduledDialog item={editing.item} onClose={() => setEditing(null)} /> : null}
      <ConfirmDialog
        busy={discarding !== null && busy === discarding.id}
        confirmLabel={discarding?.status === 'failed' ? 'Discard message' : 'Cancel message'}
        onClose={() => setDiscarding(null)}
        onConfirm={() => {
          const item = discarding
          if (!item) return
          void act(item.id, () => cancelScheduled(item.id), {
            done: item.status === 'failed' ? 'Message discarded' : 'Message canceled',
            failed: item.status === 'failed' ? 'It wasn’t discarded' : 'It wasn’t canceled',
          }).then(() => setDiscarding(null))
        }}
        open={discarding !== null}
        title={discarding?.status === 'failed' ? 'Discard this message?' : 'Cancel this message?'}
      >
        {discarding ? `“${plainPreview(discarding.plainText, 120)}” won’t be sent, and its text can’t be brought back. Copy it first if you need it.` : null}
      </ConfirmDialog>
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
      showToast({ tone: 'success', title: 'Scheduled message saved' })
      onClose()
    } catch (reason) {
      setError(describeError(reason).message)
    } finally {
      setBusy(false)
    }
  }
  return (
    <Dialog
      description={`Sends to ${item.place} ${whenPhrase(item.sendAt)}.`}
      dirty={text !== item.plainText}
      footer={
        <>
          <Button onClick={onClose} variant="secondary">
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
