import { useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { Banner, Button, Dialog, SelectField, TextField, describeError } from '../ui'
import { clearStatus, setStatus, statusKeys, type AccountStatus } from './api'
import { clearAtFor } from './statusWords'

const CLEAR_AFTER = [
  { value: '', label: 'Don’t clear' },
  { value: '30', label: 'In 30 minutes' },
  { value: '60', label: 'In an hour' },
  { value: '240', label: 'In 4 hours' },
  { value: 'today', label: 'At the end of today' },
] as const

/** Sets or clears your status: shown next to your name in every workspace you're in. */
export function StatusDialog({
  current,
  onClose,
  open,
  workspaceId,
}: {
  current: AccountStatus | null
  onClose: () => void
  open: boolean
  workspaceId: string
}) {
  const queryClient = useQueryClient()
  const [emoji, setEmoji] = useState(current?.emoji ?? '')
  const [text, setText] = useState(current?.text ?? '')
  const [clearAfter, setClearAfter] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const done = async () => {
    await queryClient.invalidateQueries({ queryKey: statusKeys.all })
    onClose()
  }
  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (busy || (!emoji.trim() && !text.trim())) return
    setBusy(true)
    setError(null)
    void setStatus(workspaceId, {
      text: text.trim() || undefined,
      emoji: emoji.trim() || undefined,
      clearAt: clearAtFor(clearAfter),
    })
      .then(done)
      .catch((reason: unknown) => setError(describeError(reason).message))
      .finally(() => setBusy(false))
  }
  const clear = () => {
    setBusy(true)
    setError(null)
    void clearStatus(workspaceId)
      .then(done)
      .catch((reason: unknown) => setError(describeError(reason).message))
      .finally(() => setBusy(false))
  }
  return (
    <Dialog
      description="It shows next to your name in every workspace you’re in."
      dirty={emoji !== (current?.emoji ?? '') || text !== (current?.text ?? '')}
      footer={
        <>
          {current ? (
            <Button disabled={busy} onClick={clear} variant="secondary">
              Clear status
            </Button>
          ) : null}
          <Button onClick={onClose} variant="secondary">
            Cancel
          </Button>
          <Button busy={busy} disabled={!emoji.trim() && !text.trim()} type="submit" variant="primary">
            Save
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open={open}
      title="Set a status"
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
      <TextField label="Emoji" maxLength={16} onChange={(event) => setEmoji(event.target.value)} placeholder="📅" value={emoji} />
      <TextField
        autoFocus
        label="Status"
        maxLength={120}
        onChange={(event) => setText(event.target.value)}
        placeholder="In a meeting, Out sick, Heads down…"
        value={text}
      />
      <SelectField label="Clear it" onChange={(event) => setClearAfter(event.target.value)} value={clearAfter}>
        {CLEAR_AFTER.map((choice) => (
          <option key={choice.value} value={choice.value}>
            {choice.label}
          </option>
        ))}
      </SelectField>
    </Dialog>
  )
}
