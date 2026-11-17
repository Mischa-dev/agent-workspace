import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'

import type { DashboardEntry, TileEntry, TileInput } from '../api/generated/operations'
import { opQuery, useOp } from '../api/ops'
import { useCollections } from '../collections/api'
import { Banner, Button, Dialog, SegmentedControl, SelectField, Switch, TextArea, TextField, describeError } from '../ui'
import { TILE_WIDTHS, widthLabel } from './dashboardModel'

/** Makes a dashboard, or changes one's name and description. */
export function DashboardDetailsDialog({
  canShare,
  dashboard,
  onClose,
  onSaved,
  projectId,
}: {
  canShare: boolean
  dashboard: DashboardEntry | null
  onClose: () => void
  onSaved: (saved: DashboardEntry) => void
  projectId: string
}) {
  const [name, setName] = useState(dashboard?.name ?? '')
  const [description, setDescription] = useState(dashboard?.description ?? '')
  const [shared, setShared] = useState(canShare)
  const [problem, setProblem] = useState<string | null>(null)
  const create = useOp('dashboard_create', { invalidates: ['dashboard_list'] })
  const update = useOp('dashboard_update', { invalidates: ['dashboard_list', 'dashboard_get'] })
  const busy = create.isPending || update.isPending
  const submit = async () => {
    setProblem(null)
    if (!name.trim()) return setProblem('Give the dashboard a name.')
    try {
      const saved = dashboard
        ? await update.mutateAsync({ dashboard: dashboard.id, name: name.trim(), description: description.trim(), expectedVersion: dashboard.version })
        : await create.mutateAsync({ spaceId: projectId, name: name.trim(), description: description.trim() || null, shared })
      onSaved(saved)
      onClose()
    } catch (error) {
      setProblem(describeError(error).message)
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
            {dashboard ? 'Save' : 'Make dashboard'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        void submit()
      }}
      open
      title={dashboard ? `Edit ${dashboard.name}` : 'New dashboard'}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      <TextField
        autoFocus
        hint={dashboard ? undefined : 'Such as Weekly numbers, Playtest results or Release health.'}
        label="Name"
        maxLength={80}
        onChange={(event) => setName(event.target.value)}
        required
        value={name}
      />
      <TextField hint="Optional." label="Description" maxLength={500} onChange={(event) => setDescription(event.target.value)} value={description} />
      {dashboard ? null : (
        <Switch
          checked={shared}
          description={canShare ? 'Everyone in the project sees it, each reading only the records they can see. Off, it’s yours alone.' : 'Your role keeps dashboards of its own. Someone who can edit dashboards can share one.'}
          disabled={!canShare}
          label="Share with the project"
          onChange={setShared}
        />
      )}
    </Dialog>
  )
}

type Kind = 'view' | 'text'

/**
 * Adds a tile, or changes one: a view of one of the project's collections
 * (a chart view shows as its chart), or a note. A shared dashboard offers
 * shared views only.
 */
export function TileDialog({
  onClose,
  onSave,
  projectId,
  shared,
  tile,
}: {
  onClose: () => void
  onSave: (tile: TileInput) => Promise<void>
  projectId: string
  shared: boolean
  tile: TileEntry | null
}) {
  const [kind, setKind] = useState<Kind>(tile?.kind ?? 'view')
  const [collection, setCollection] = useState(tile?.collection ?? '')
  const [view, setView] = useState(tile?.view ?? '')
  const [title, setTitle] = useState(tile?.title ?? '')
  const [text, setText] = useState(tile?.text ?? '')
  const [width, setWidth] = useState(tile?.width ?? 6)
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  const collections = useCollections(projectId)
  const views = useQuery({ ...opQuery('view_list', { collection }), enabled: kind === 'view' && Boolean(collection) })
  const offered = (views.data?.views ?? []).filter((entry) => !shared || entry.shared)
  const submit = async () => {
    setProblem(null)
    if (kind === 'view' && !view) return setProblem('Pick a view to show.')
    if (kind === 'text' && !text.trim()) return setProblem('Write the note.')
    setBusy(true)
    try {
      await onSave({
        id: tile?.id ?? null,
        kind,
        title: title.trim() || null,
        view: kind === 'view' ? view : null,
        text: kind === 'text' ? text.trim() : null,
        width,
        height: tile?.height ?? (kind === 'text' ? 1 : 2),
      })
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
            {tile ? 'Save' : 'Add tile'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        void submit()
      }}
      open
      title={tile ? 'Edit tile' : 'Add a tile'}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      {tile ? null : (
        <SegmentedControl<Kind>
          label="What the tile shows"
          onChange={setKind}
          options={[
            { value: 'view', label: 'A view' },
            { value: 'text', label: 'A note' },
          ]}
          value={kind}
        />
      )}
      {kind === 'view' ? (
        <>
          <SelectField
            label="Collection"
            onChange={(event) => {
              setCollection(event.target.value)
              setView('')
            }}
            required
            value={collection}
          >
            <option value="">{collections.isPending ? 'Loading…' : 'Pick a collection'}</option>
            {(collections.data?.collections ?? [])
              .filter((entry) => !entry.archived)
              .map((entry) => (
                <option key={entry.id} value={entry.id}>
                  {entry.name}
                </option>
              ))}
          </SelectField>
          <SelectField
            disabled={!collection}
            hint={shared ? 'A shared dashboard shows shared views. Make a chart view in the collection to see it here as a chart.' : 'Make a chart view in the collection to see it here as a chart.'}
            label="View"
            onChange={(event) => setView(event.target.value)}
            required
            value={view}
          >
            <option value="">{views.isFetching ? 'Loading…' : offered.length || !collection ? 'Pick a view' : 'No views to show'}</option>
            {offered.map((entry) => (
              <option key={entry.id} value={entry.id}>
                {entry.name}
                {entry.type === 'chart' ? ' (chart)' : ''}
                {entry.shared ? '' : ' (yours)'}
              </option>
            ))}
          </SelectField>
        </>
      ) : (
        <TextArea label="Note" maxLength={2000} onChange={(event) => setText(event.target.value)} required rows={4} value={text} />
      )}
      <TextField hint={kind === 'view' ? 'Optional; the view’s name when left empty.' : 'Optional.'} label="Title" maxLength={80} onChange={(event) => setTitle(event.target.value)} value={title} />
      <SelectField label="Width" onChange={(event) => setWidth(Number(event.target.value))} value={String(width)}>
        {TILE_WIDTHS.map((option) => (
          <option key={option} value={option}>
            {widthLabel(option)}
          </option>
        ))}
      </SelectField>
    </Dialog>
  )
}
