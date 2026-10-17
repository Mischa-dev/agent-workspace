import { useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import type { ConversationMessage } from '../conversations/types'
import { Banner, Button, Checkbox, Dialog, Icon, IconButton, TextArea, TextField } from '../ui'
import { memoryKeys, pin, recordDecision, unpin, type Pin } from './api'
import { usePins } from './hooks'

/** What's pinned in a conversation: the pages, decisions and messages that are true here now. */
export function PinnedBar({
  canEdit,
  conversationId,
  direct = false,
  projectId,
}: {
  canEdit: boolean
  conversationId: string
  /** DMs and groups have no project pages to link to. */
  direct?: boolean
  projectId: string
}) {
  const pins = usePins(projectId, conversationId)
  const queryClient = useQueryClient()
  const [open, setOpen] = useState(false)
  const items = pins.data?.pins ?? []
  if (items.length === 0) return null
  const href = (item: Pin) =>
    item.kind === 'page'
      ? `/projects/${encodeURIComponent(projectId)}/pages/${encodeURIComponent(item.id)}`
      : item.kind === 'decision'
        ? `/projects/${encodeURIComponent(projectId)}/decisions#${encodeURIComponent(item.id)}`
        : `#message-${item.id}`
  return (
    <div className="aw-pins">
      <button aria-expanded={open} className="aw-pins__toggle" onClick={() => setOpen(!open)} type="button">
        <Icon name="star" size={14} />
        {items.length === 1 ? '1 pinned' : `${items.length} pinned`}
        <Icon name="chevron-down" size={12} />
      </button>
      {open ? (
        <ul className="aw-pins__list">
          {items.map((item) => (
            <li className="aw-pins__item" key={`${item.kind}:${item.id}`}>
              {item.kind === 'message' ? (
                <a className="aw-pins__title" href={href(item)}>
                  {item.title}
                </a>
              ) : direct ? (
                <strong className="aw-pins__title">{item.title}</strong>
              ) : (
                <Link className="aw-pins__title" to={href(item)}>
                  {item.title}
                </Link>
              )}
              <span className="aw-pins__text">{item.text}</span>
              {canEdit ? (
                <IconButton
                  icon="x"
                  label={`Unpin ${item.title}`}
                  onClick={() =>
                    void unpin(conversationId, item.kind, item.id).then(() =>
                      queryClient.invalidateQueries({ queryKey: memoryKeys.pins(projectId, conversationId) }),
                    )
                  }
                  size="sm"
                />
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  )
}

/** Pin and "Save as decision" buttons for a message's toolbar. */
export function MessageMemoryActions({
  conversationId,
  message,
  projectId,
}: {
  conversationId: string
  message: ConversationMessage
  projectId: string
}) {
  const queryClient = useQueryClient()
  const [deciding, setDeciding] = useState(false)
  return (
    <>
      <IconButton
        icon="star"
        label="Pin to this conversation"
        onClick={() =>
          void pin(conversationId, 'message', message.id).then(() =>
            queryClient.invalidateQueries({ queryKey: memoryKeys.pins(projectId, conversationId) }),
          )
        }
        size="sm"
      />
      <IconButton icon="check" label="Save as decision" onClick={() => setDeciding(true)} size="sm" />
      {deciding ? (
        <DecisionDialog
          conversationId={conversationId}
          message={message}
          onClose={() => setDeciding(false)}
          projectId={projectId}
        />
      ) : null}
    </>
  )
}

function firstLine(text: string) {
  const line = text.trim().split('\n')[0] ?? ''
  return line.length > 200 ? `${line.slice(0, 199)}…` : line
}

function DecisionDialog({
  conversationId,
  message,
  onClose,
  projectId,
}: {
  conversationId: string
  message: ConversationMessage
  onClose: () => void
  projectId: string
}) {
  const queryClient = useQueryClient()
  const [title, setTitle] = useState(firstLine(message.plainText))
  const [detail, setDetail] = useState('')
  const [pinIt, setPinIt] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!title.trim() || busy) return
    setBusy(true)
    setError(null)
    void recordDecision(projectId, { title: title.trim(), detail: detail.trim() || undefined, sourceMessageId: message.id })
      .then(async (decision) => {
        if (pinIt) await pin(conversationId, 'decision', decision.id)
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: memoryKeys.decisions(projectId) }),
          queryClient.invalidateQueries({ queryKey: memoryKeys.pins(projectId, conversationId) }),
        ])
        onClose()
      })
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : 'The decision wasn’t saved.'))
      .finally(() => setBusy(false))
  }
  return (
    <Dialog
      description="Decisions are kept with the project, so people and agents can find what was settled and why without rereading the chat."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} disabled={!title.trim()} type="submit" variant="primary">
            Save decision
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title="Save as decision"
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
      <TextField label="What was decided" maxLength={300} onChange={(event) => setTitle(event.target.value)} value={title} />
      <TextArea label="Why (optional)" maxLength={20000} onChange={(event) => setDetail(event.target.value)} rows={4} value={detail} />
      <Checkbox checked={pinIt} label="Pin it to this conversation" onChange={setPinIt} />
    </Dialog>
  )
}
