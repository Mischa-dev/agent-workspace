import { useState, type FormEvent } from 'react'

import { isApiError } from '../api/errors'
import { Banner, Button, Dialog, TextField, formatFullDate } from '../ui'
import { parseLeadingWhen, toLocalInput, whenChoices } from './when'

/** Outside the component: the time is read when it's submitted. */
function isPast(at: Date) {
  return at.getTime() <= Date.now()
}

function failure(reason: unknown, fallback: string) {
  return isApiError(reason) ? reason.message : reason instanceof Error ? reason.message : fallback
}

/** Pick a custom time: a date and time field, or words like "friday 3pm". */
export function TimeDialog({
  action,
  onClose,
  onPick,
  title,
  description,
  noteLabel,
  initialAt,
}: {
  action: string
  onClose: () => void
  /** `note` is the text typed in the note field, when there is one. */
  onPick: (at: Date, note: string) => Promise<void> | void
  title: string
  description?: string
  /** Also asks for words, such as what to be reminded of. */
  noteLabel?: string
  initialAt?: Date
}) {
  const [note, setNote] = useState('')
  const [value, setValue] = useState(() => toLocalInput(initialAt ?? whenChoices()[2].at))
  const [words, setWords] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const typed = words.trim() ? parseLeadingWhen(words.trim()) : null
  const typedAt = typed && typed.length === words.trim().length ? typed.at : null

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (noteLabel && !note.trim()) {
      setError('Say what it’s about.')
      return
    }
    const at = words.trim() ? typedAt : new Date(value)
    if (!at || Number.isNaN(at.getTime())) {
      setError(words.trim() ? 'Try something like “tomorrow 3pm” or “in 2 hours”.' : 'Pick a date and time.')
      return
    }
    if (isPast(at)) {
      setError('Pick a time in the future.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      await onPick(at, note.trim())
      onClose()
    } catch (reason) {
      setError(failure(reason, 'That didn’t work. Try again.'))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      description={description}
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={busy} type="submit" variant="primary">
            {action}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => void submit(event)}
      open
      title={title}
      width={420}
    >
      {noteLabel ? (
        <TextField data-autofocus label={noteLabel} maxLength={1000} onChange={(event) => setNote(event.target.value)} value={note} />
      ) : null}
      <TextField
        data-autofocus={noteLabel ? undefined : true}
        hint={words.trim() ? (typedAt ? formatFullDate(typedAt.toISOString()) : 'Not a time yet') : 'Or type it, like “friday 3pm”.'}
        label="When"
        onChange={(event) => setWords(event.target.value)}
        placeholder="tomorrow at 3pm"
        value={words}
      />
      <TextField
        disabled={Boolean(words.trim())}
        label="Date and time"
        onChange={(event) => setValue(event.target.value)}
        type="datetime-local"
        value={value}
      />
      {error ? <Banner tone="danger">{error}</Banner> : null}
    </Dialog>
  )
}
