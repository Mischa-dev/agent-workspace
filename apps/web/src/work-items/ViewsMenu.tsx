import { useRef, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { useProjectExperience } from '../experience/layout'
import { Button, ConfirmDialog, Dialog, Menu, describeError, showToast, Switch, TextField, type MenuEntry } from '../ui'
import { createView, deleteView, listViews, pinView, updateView, viewKeys, type SavedView, type ViewDraft } from '../views/api'
import { COLUMN_PARTS, viewName, viewShown } from '../views/layoutViews'
import { VIEW_FIELDS, readListState, writeListState, type ListState } from './listModel'
import type { UpdateListState } from './useListState'

const FIELD_LABELS: Record<string, string> = {
  priority: 'Priority',
  key: 'Key',
  status: 'Status',
  details: 'Labels and details',
  due: 'Due date',
  assignee: 'Assignee',
}

/** The view as it would be saved: filters, sort, grouping and board setup, without the open task. */
function draftFrom(state: ListState, actorId: string): Omit<ViewDraft, 'name' | 'shared'> {
  const parameters = writeListState({ ...state, task: null, savedView: null, fields: null, view: 'list', showDone: false })
  const query = Object.fromEntries(parameters.entries())
  // "Me" follows whoever opens the view, so a shared "assigned to me" view works for everyone.
  for (const key of ['assigneeActorId', 'notAssigneeActorId']) {
    if (query[key]) query[key] = query[key].split(',').map((id) => (id === actorId ? 'me' : id)).join(',')
  }
  return { query, layout: state.view, fields: state.fields }
}

/** A query as the list reads it, so an older spelling of the same filters still matches. */
function readQuery(query: Record<string, string>) {
  const read = writeListState({ ...readListState(new URLSearchParams(query)), task: null, savedView: null, fields: null, view: 'list', showDone: false })
  return [...read.entries()].sort().join('&')
}

function sameQuery(view: SavedView, draft: Omit<ViewDraft, 'name' | 'shared'>) {
  return readQuery(view.query) === readQuery(draft.query) && view.layout === draft.layout && (view.fields ?? []).join(',') === (draft.fields ?? []).join(',')
}

/**
 * The Views button on the tasks page: open a saved view, save the current
 * one, pin it to the sidebar, and pick the list's columns.
 */
export default function ViewsMenu({
  actorId,
  projectId,
  readOnly,
  state,
  update,
}: {
  actorId: string
  projectId: string
  readOnly: boolean
  state: ListState
  update: UpdateListState
}) {
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const views = useQuery({ queryKey: viewKeys.views(projectId), queryFn: () => listViews(projectId) })
  const experience = useProjectExperience(projectId)
  const [saving, setSaving] = useState<'new' | 'rename' | null>(null)
  const [deleting, setDeleting] = useState<SavedView | null>(null)
  const current = views.data?.views.find((view) => view.id === state.savedView) ?? null
  const draft = draftFrom(state, actorId)
  const changed = current ? !sameQuery(current, draft) : false
  const refresh = () => queryClient.invalidateQueries({ queryKey: viewKeys.views(projectId) })
  const failed = (title: string) => (reason: unknown) => showToast({ tone: 'danger', title, description: describeError(reason).message })

  const save = useMutation({
    mutationFn: (view: SavedView) => updateView(projectId, view, draft, crypto.randomUUID()),
    onSuccess: (view) => {
      void refresh()
      showToast({ tone: 'success', title: `Saved “${view.name}”` })
    },
    onError: failed('The view didn’t save'),
  })
  const pin = useMutation({
    mutationFn: (view: SavedView) => pinView(projectId, view.id, !view.pinned, crypto.randomUUID()),
    onSuccess: (view) => {
      void refresh()
      showToast({ title: view.pinned ? `“${view.name}” is in the sidebar` : `“${view.name}” left the sidebar` })
    },
    onError: failed('The sidebar didn’t change'),
  })
  const remove = useMutation({
    mutationFn: (view: SavedView) => deleteView(projectId, view, crypto.randomUUID()),
    onSuccess: (_result, view) => {
      setDeleting(null)
      void refresh()
      update((state) => ({ ...state, savedView: null }))
      showToast({ title: `Deleted “${view.name}”` })
    },
  })

  const items: MenuEntry[] = []
  if (views.isError) items.push({ label: 'Views didn’t load. Try again', icon: 'retry', keepOpen: true, onSelect: () => void views.refetch() })
  for (const view of views.data?.views ?? []) {
    if (!viewShown(view, experience) && view.id !== state.savedView) continue
    items.push({
      label: view.shared || view.builtin ? viewName(view, experience) : `${view.name} (only you)`,
      checked: view.id === state.savedView,
      onSelect: () => navigate(view.href),
    })
  }
  if (!readOnly) {
    items.push({ separator: true })
    if (current?.canEdit && changed) items.push({ label: `Save changes to “${current.name}”`, icon: 'check', onSelect: () => save.mutate(current) })
    items.push({ label: 'Save as new view…', icon: 'plus', onSelect: () => setSaving('new') })
  }
  if (current) {
    items.push({ label: current.pinned ? 'Unpin from sidebar' : 'Pin to sidebar', icon: 'star', onSelect: () => pin.mutate(current) })
    if (current.canEdit && !readOnly) items.push({ label: 'Rename…', icon: 'pencil', onSelect: () => setSaving('rename') })
    if (current.canDelete && !readOnly) items.push({ label: 'Delete view…', icon: 'trash', danger: true, onSelect: () => setDeleting(current) })
  }
  if (state.view === 'list') {
    items.push({ heading: 'Columns' })
    const shown = state.fields ?? VIEW_FIELDS
    for (const field of VIEW_FIELDS) {
      const part = COLUMN_PARTS[field]
      if (part && !experience.shows(part)) continue
      items.push({
        label: FIELD_LABELS[field] ?? field,
        checked: shown.includes(field),
        keepOpen: true,
        onSelect: () => {
          const next = shown.includes(field) ? shown.filter((entry) => entry !== field) : VIEW_FIELDS.filter((entry) => entry === field || shown.includes(entry))
          update((state) => ({ ...state, fields: next.length === VIEW_FIELDS.length ? null : next }), { replace: true })
        },
      })
    }
  }

  return (
    <>
      <Menu
        items={items}
        label="Views"
        trigger={
          <Button iconEnd="chevron-down" variant="secondary">
            {current ? `${current.name}${changed ? ' (changed)' : ''}` : 'Views'}
          </Button>
        }
      />
      {saving ? (
        <SaveViewDialog
          canShare={Boolean(views.data?.canShare)}
          draft={draft}
          onClose={() => setSaving(null)}
          onSaved={(view) => {
            setSaving(null)
            void refresh()
            navigate(view.href, { replace: true })
          }}
          projectId={projectId}
          renaming={saving === 'rename' ? current : null}
        />
      ) : null}
      <ConfirmDialog
        busy={remove.isPending}
        confirmLabel="Delete view"
        error={remove.isError ? describeError(remove.error).message : undefined}
        onClose={() => {
          setDeleting(null)
          remove.reset()
        }}
        onConfirm={() => deleting && remove.mutate(deleting)}
        open={deleting !== null}
        title={`Delete “${deleting?.name ?? ''}”?`}
      >
        {deleting?.shared ? 'Everyone who uses this view loses it. Its tasks stay as they are.' : 'Its tasks stay as they are.'}
      </ConfirmDialog>
    </>
  )
}

function SaveViewDialog({
  canShare,
  draft,
  onClose,
  onSaved,
  projectId,
  renaming,
}: {
  canShare: boolean
  draft: Omit<ViewDraft, 'name' | 'shared'>
  onClose: () => void
  onSaved: (view: SavedView) => void
  projectId: string
  renaming: SavedView | null
}) {
  const idempotencyKey = useRef(crypto.randomUUID())
  const [name, setName] = useState(renaming?.name ?? '')
  const [shared, setShared] = useState(renaming?.shared ?? false)
  const [pinned, setPinned] = useState(renaming?.pinned ?? true)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim() || busy) return
    setBusy(true)
    setError(null)
    try {
      let view = renaming
        ? await updateView(projectId, renaming, { name: name.trim(), shared }, idempotencyKey.current)
        : await createView(projectId, { ...draft, name: name.trim(), shared }, idempotencyKey.current)
      if (view.pinned !== pinned) view = await pinView(projectId, view.id, pinned, crypto.randomUUID())
      onSaved(view)
    } catch (reason) {
      idempotencyKey.current = crypto.randomUUID()
      setError(describeError(reason).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      description={renaming ? undefined : 'Saves the filters, sort, grouping, layout and columns you have now.'}
      footer={
        <>
          <Button onClick={onClose} variant="secondary">
            Cancel
          </Button>
          <Button busy={busy} disabled={!name.trim()} type="submit" variant="primary">
            {renaming ? 'Save' : 'Save view'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => void submit(event)}
      open
      title={renaming ? 'Rename view' : 'Save view'}
    >
      <TextField
        autoComplete="off"
        autoFocus
        error={error ?? undefined}
        label="Name"
        maxLength={80}
        onChange={(event) => setName(event.target.value)}
        placeholder="e.g. Release blockers"
        value={name}
      />
      {canShare && !renaming?.builtin ? (
        <Switch checked={shared} description="Everyone in the project sees it. Otherwise it’s yours alone." label="Share with the project" onChange={setShared} />
      ) : null}
      <Switch checked={pinned} description="One click from the project’s sidebar." label="Pin to sidebar" onChange={setPinned} />
    </Dialog>
  )
}
