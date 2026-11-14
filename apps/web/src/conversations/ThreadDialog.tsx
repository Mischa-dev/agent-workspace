import { useRef, useState, type FormEvent } from 'react'

import { Button, Dialog, TextField, describeError } from '../ui'
import { createThread } from './api'
import '../styles/conversation.css'

function suggestedName(text: string) {
  const firstLine = text.trim().split('\n')[0] ?? ''
  const words = [...firstLine].slice(0, 60).join('')
  return words.length < firstLine.length ? `${words.replace(/\s+\S*$/, '')}…` : words
}

/** Name a new thread, optionally started from a message. Mount it when it opens so the name starts fresh. */
export function ThreadDialog({
  channelId,
  onClose,
  onCreated,
  open,
  origin,
}: {
  channelId: string
  onClose: () => void
  onCreated: (threadId: string) => void
  open: boolean
  origin?: { messageId: string; text: string; author: string } | null
}) {
  const keyRef = useRef(crypto.randomUUID())
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const title = name.trim()
    if (busy) return
    setBusy(true)
    setError(null)
    void createThread({ channelId, name: title || undefined, originMessageId: origin?.messageId, idempotencyKey: keyRef.current })
      .then((thread) => {
        onClose()
        onCreated(thread.id)
      })
      .catch((reason: unknown) => setError(`The thread wasn’t started. ${describeError(reason).message}`))
      .finally(() => setBusy(false))
  }

  return (
    <Dialog
      description={
        origin
          ? `Replies to ${origin.author}’s message go in the thread, so the channel stays readable. Agents can run work there too.`
          : 'A thread is a focused conversation inside the channel. Agents can run work there too.'
      }
      onClose={onClose}
      open={open}
      title="Start a thread"
    >
      <form className="aw-form" onSubmit={submit}>
        {origin ? <blockquote className="aw-quote">{origin.text}</blockquote> : null}
        <TextField
          autoComplete="off"
          error={error ?? undefined}
          hint="Optional. Until someone names it, the thread goes by its first message."
          label="Thread name"
          maxLength={120}
          onChange={(event) => setName(event.target.value)}
          placeholder={origin ? suggestedName(origin.text) : 'Named after its first message'}
          value={name}
        />
        <div className="aw-form__actions">
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} type="submit" variant="primary">
            Start thread
          </Button>
        </div>
      </form>
    </Dialog>
  )
}
