import { useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { useProjects } from '../projects/hooks'
import { Banner, Button, Dialog, SelectField, TextField, describeError } from '../ui'
import {
  createWhiteboard,
  updateWhiteboard,
  whiteboardKeys,
  whiteboardPath,
  type LinkTargetKind,
  type Whiteboard,
  type Workspace,
} from './api'

/**
 * Makes a board. From the Whiteboards page it asks where the board lives;
 * from a task, project or page it lives in that project and is linked there.
 */
export function NewWhiteboardDialog({
  link,
  onClose,
  open,
  projectId,
  workspaces = [],
}: {
  link?: { targetKind: LinkTargetKind; targetId: string }
  onClose: () => void
  open: boolean
  /** Make it in this project, without asking. */
  projectId?: string
  workspaces?: Workspace[]
}) {
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [title, setTitle] = useState('')
  const [place, setPlace] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const fallback = workspaces[0] ? `workspace:${workspaces[0].id}` : ''
  const chosen = place || fallback
  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (!title.trim() || busy) return
    const [kind, id] = chosen.split(':')
    setBusy(true)
    setError(null)
    void createWhiteboard({
      title: title.trim(),
      ...(projectId ? { projectId } : kind === 'project' ? { projectId: id } : { organizationId: id }),
      ...(link ? { link } : {}),
    })
      .then((created) => {
        void queryClient.invalidateQueries({ queryKey: whiteboardKeys.all })
        setTitle('')
        onClose()
        navigate(whiteboardPath(created.whiteboard.id))
      })
      .catch((reason: unknown) => setError(describeError(reason).message))
      .finally(() => setBusy(false))
  }
  return (
    <Dialog
      description={link ? 'It’s linked here and opens to draw on. You can link it to more tasks and pages later.' : undefined}
      dirty={Boolean(title.trim())}
      footer={
        <>
          <Button onClick={onClose} variant="secondary">
            Cancel
          </Button>
          <Button busy={busy} disabled={!title.trim()} type="submit" variant="primary">
            Create whiteboard
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open={open}
      title="New whiteboard"
    >
      <TextField
        autoFocus
        label="Title"
        maxLength={200}
        onChange={(event) => setTitle(event.target.value)}
        placeholder="Launch plan, Architecture, Retro…"
        value={title}
      />
      {projectId ? null : <PlacePicker onChange={setPlace} value={chosen} workspaces={workspaces} />}
      {error ? <Banner tone="danger">{error}</Banner> : null}
    </Dialog>
  )
}

/** Who can see a new board: everyone in a workspace, or one project's members. */
function PlacePicker({
  onChange,
  value,
  workspaces,
}: {
  onChange: (value: string) => void
  value: string
  workspaces: Workspace[]
}) {
  const projects = useProjects()
  const choices = [
    ...workspaces.map((workspace) => ({ value: `workspace:${workspace.id}`, label: `Everyone in ${workspace.name}` })),
    ...(projects.data?.projects ?? [])
      .filter((project) => !project.archivedAt)
      .map((project) => ({ value: `project:${project.id}`, label: `${project.name} members` })),
  ]
  return (
    <SelectField
      hint="A project’s whiteboards are only for its members."
      label="Who can see it"
      onChange={(event) => onChange(event.target.value)}
      value={value}
    >
      {choices.map((choice) => (
        <option key={choice.value} value={choice.value}>
          {choice.label}
        </option>
      ))}
    </SelectField>
  )
}

export function RenameDialog({ board, onClose, open }: { board: Whiteboard; onClose: () => void; open: boolean }) {
  const queryClient = useQueryClient()
  const [title, setTitle] = useState(board.title)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (!title.trim() || busy) return
    setBusy(true)
    setError(null)
    void updateWhiteboard(board.id, { title: title.trim() })
      .then(() => {
        void queryClient.invalidateQueries({ queryKey: whiteboardKeys.all })
        onClose()
      })
      .catch((reason: unknown) => setError(describeError(reason).message))
      .finally(() => setBusy(false))
  }
  return (
    <Dialog
      dirty={title.trim() !== board.title}
      footer={
        <>
          <Button onClick={onClose} variant="secondary">
            Cancel
          </Button>
          <Button busy={busy} disabled={!title.trim() || title.trim() === board.title} type="submit" variant="primary">
            Rename
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open={open}
      title="Rename whiteboard"
    >
      <TextField autoFocus label="Title" maxLength={200} onChange={(event) => setTitle(event.target.value)} value={title} />
      {error ? <Banner tone="danger">{error}</Banner> : null}
    </Dialog>
  )
}
