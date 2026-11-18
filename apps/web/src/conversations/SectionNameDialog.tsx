import { useState, type FormEvent } from 'react'

import { Button, Dialog, TextField, describeError } from '../ui'

/** Names a new sidebar section, or renames one (C2.9). */
export function SectionNameDialog({
  initial = '',
  onClose,
  onSave,
  title,
}: {
  initial?: string
  onClose: () => void
  /** Saves the name; a refusal (such as a name already used) shows on the field. */
  onSave: (name: string) => Promise<unknown>
  title: string
}) {
  const [value, setValue] = useState(initial)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const name = value.trim()

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (busy) return
    if (!name) return setError('Give the section a name.')
    if (name === initial) return onClose()
    setBusy(true)
    setError(null)
    void onSave(name)
      .then(onClose)
      .catch((reason: unknown) => setError(describeError(reason).message))
      .finally(() => setBusy(false))
  }

  return (
    <Dialog
      dirty={name !== initial}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} type="submit" variant="primary">
            Save
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title={title}
    >
      <TextField
        autoComplete="off"
        autoFocus
        error={error ?? undefined}
        label="Name"
        maxLength={80}
        onChange={(event) => setValue(event.target.value)}
        value={value}
      />
    </Dialog>
  )
}
