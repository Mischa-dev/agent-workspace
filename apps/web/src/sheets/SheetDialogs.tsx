import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { Banner, Button, Dialog, TextField, describeError } from '../ui'
import { createSheet, sheetKeys, updateSheet, type SheetBody, type SheetSummary } from './api'

/** Makes a sheet, or changes one's name and description. */
export function SheetDetailsDialog({
  onClose,
  onSaved,
  projectId,
  sheet,
  tab,
}: {
  onClose: () => void
  onSaved: (body: SheetBody) => void
  projectId: string
  sheet: SheetSummary | null
  /** The tab on screen, whose cells come back with the change. */
  tab?: string
}) {
  const client = useQueryClient()
  const [name, setName] = useState(sheet?.name ?? '')
  const [description, setDescription] = useState(sheet?.description ?? '')
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  const submit = async () => {
    setProblem(null)
    if (!name.trim()) return setProblem('Give the sheet a name.')
    setBusy(true)
    const details = { name: name.trim(), description: description.trim() }
    try {
      const body = sheet
        ? await updateSheet(projectId, sheet.id, { ...details, tab })
        : await createSheet(projectId, {
            ...details,
            idempotencyKey: crypto.randomUUID(),
          })
      void client.invalidateQueries({ queryKey: sheetKeys.all(projectId) })
      onSaved(body)
      onClose()
    } catch (error) {
      setProblem(describeError(error).message)
      setBusy(false)
    }
  }
  return (
    <Dialog
      dirty
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={busy} type="submit" variant="primary">
            {sheet ? 'Save' : 'Make sheet'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        void submit()
      }}
      open
      title={sheet ? `Edit ${sheet.name}` : 'New sheet'}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      <TextField
        autoFocus
        hint={sheet ? undefined : 'Such as Budget, Playtest schedule or Asset list.'}
        label="Name"
        maxLength={80}
        onChange={(event) => setName(event.target.value)}
        required
        value={name}
      />
      <TextField hint="Optional." label="Description" maxLength={500} onChange={(event) => setDescription(event.target.value)} value={description} />
    </Dialog>
  )
}

/** Names a new tab, or renames one. */
export function TabNameDialog({
  busy,
  initial,
  onClose,
  onSave,
  problem,
  title,
}: {
  busy: boolean
  initial: string
  onClose: () => void
  onSave: (name: string) => void
  problem: string | null
  title: string
}) {
  const [name, setName] = useState(initial)
  return (
    <Dialog
      dirty
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={busy} type="submit" variant="primary">
            Save
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        if (name.trim()) onSave(name.trim())
      }}
      open
      title={title}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      <TextField
        autoFocus
        hint="Up to 31 characters, without [ ] : * ? / or \."
        label="Tab name"
        maxLength={31}
        onChange={(event) => setName(event.target.value)}
        required
        value={name}
      />
    </Dialog>
  )
}
