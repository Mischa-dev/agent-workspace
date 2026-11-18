import { useState, type FormEvent } from 'react'

import { useWorkspaceId } from '../app/workspace'
import { useProjects } from '../projects/hooks'
import { Banner, Button, Dialog, IconButton, Menu, SelectField, TextField, describeError, showToast, type MenuEntry } from '../ui'
import { currentView, useInboxViews, useSetInboxView, type InboxDensity, type InboxFilter, type InboxSavedView } from './views'

/**
 * The inbox's saved views as tabs (C2.11), with the spaces filter beside
 * them. Picking a tab puts its filter in the address; the tab whose filter
 * is on screen is the current one, and its menu renames, moves, sets its
 * density or removes it.
 */
export function InboxViewTabs({
  filter,
  onApply,
  suggestedName,
}: {
  filter: InboxFilter
  onApply: (filter: InboxFilter) => void
  suggestedName: string
}) {
  const views = useInboxViews()
  const setView = useSetInboxView()
  const [saving, setSaving] = useState<'new' | InboxSavedView | null>(null)
  const list = views.data?.views ?? []
  const current = currentView(list, filter)
  const everything = filter.view === 'inbox' && filter.category === 'all' && filter.spaces.length === 0

  const run = (input: Parameters<typeof setView>[0], failed: string) =>
    setView(input).catch((reason: unknown) => showToast({ tone: 'danger', title: failed, description: describeError(reason).message }))

  const menu = (saved: InboxSavedView): MenuEntry[] => {
    const at = list.indexOf(saved)
    return [
      { label: 'Rename…', icon: 'pencil', onSelect: () => setSaving(saved) },
      {
        label: 'Compact rows',
        checked: saved.density === 'compact',
        onSelect: () => void run({ inboxViewId: saved.id, density: saved.density === 'compact' ? 'detailed' : 'compact' }, 'The view wasn’t changed'),
      },
      { label: 'Move left', disabled: at <= 0, onSelect: () => void move(at, -1) },
      { label: 'Move right', disabled: at >= list.length - 1, onSelect: () => void move(at, 1) },
      { separator: true },
      {
        label: 'Remove view',
        icon: 'trash',
        danger: true,
        onSelect: () =>
          void run({ inboxViewId: saved.id, delete: true }, 'The view wasn’t removed').then(() =>
            showToast({
              tone: 'success',
              title: `Removed “${saved.name}”`,
              action: {
                label: 'Undo',
                onSelect: () =>
                  void run(
                    { name: saved.name, filter: saved.filter, density: saved.density, position: saved.position },
                    'The view wasn’t put back',
                  ),
              },
            }),
          ),
      },
    ]
  }

  // Positions are rewritten for the two that swap, so ties can't stick.
  const move = async (at: number, by: -1 | 1) => {
    const other = list[at + by]
    const saved = list[at]
    if (!other || !saved) return
    await run({ inboxViewId: saved.id, position: at + by }, 'The view wasn’t moved')
    await run({ inboxViewId: other.id, position: at }, 'The view wasn’t moved')
  }

  return (
    <div aria-label="Inbox views" className="aw-inbox__views" role="group">
      <Button aria-pressed={everything} className="aw-inbox__view" onClick={() => onApply({ view: 'inbox', category: 'all', spaces: [] })} size="sm" variant="ghost">
        Everything
      </Button>
      {list.map((saved) => {
        const on = current?.id === saved.id
        return (
          <span className="aw-inbox__view-tab" data-current={on ? 'true' : undefined} key={saved.id}>
            <Button
              aria-pressed={on}
              className="aw-inbox__view"
              onClick={() => onApply({ view: saved.filter.view ?? 'inbox', category: saved.filter.category ?? 'all', spaces: saved.filter.spaces ?? [] })}
              size="sm"
              variant="ghost"
            >
              {saved.name}
            </Button>
            {on ? (
              <Menu items={menu(saved)} label={`View ${saved.name}`} trigger={<IconButton icon="more" label={`More for ${saved.name}`} size="sm" />} />
            ) : null}
          </span>
        )
      })}
      <SpacesFilter onChange={(spaces) => onApply({ ...filter, spaces })} spaces={filter.spaces} />
      {!current && !everything ? (
        <Button icon="plus" onClick={() => setSaving('new')} size="sm" variant="ghost">
          Save view
        </Button>
      ) : null}
      {saving ? (
        <SaveViewDialog
          filter={filter}
          onClose={() => setSaving(null)}
          saved={saving === 'new' ? null : saving}
          suggestedName={suggestedName}
        />
      ) : null}
    </div>
  )
}

/** Which spaces the inbox shows: every space, or the ones ticked. */
function SpacesFilter({ onChange, spaces }: { onChange: (spaces: string[]) => void; spaces: string[] }) {
  const workspaceId = useWorkspaceId()
  const projects = useProjects()
  const here = (projects.data?.projects ?? []).filter((project) => project.organizationId === workspaceId && project.kind !== 'workspace')
  if (here.length < 2) return null
  const toggle = (id: string) => onChange(spaces.includes(id) ? spaces.filter((space) => space !== id) : [...spaces, id].slice(0, 20))
  const label =
    spaces.length === 0
      ? 'All spaces'
      : spaces.length === 1
        ? (here.find((project) => project.id === spaces[0])?.name ?? '1 space')
        : `${spaces.length} spaces`
  return (
    <Menu
      items={[
        { label: 'All spaces', checked: spaces.length === 0, onSelect: () => onChange([]) },
        { separator: true },
        ...here.map((project) => ({ label: project.name, checked: spaces.includes(project.id), keepOpen: true, onSelect: () => toggle(project.id) })),
      ]}
      label="Show spaces"
      trigger={
        <Button aria-label={`Spaces: ${label}`} className="aw-inbox__view" iconEnd="chevron-down" size="sm" variant="ghost">
          {label}
        </Button>
      }
    />
  )
}

function SaveViewDialog({
  filter,
  onClose,
  saved,
  suggestedName,
}: {
  filter: InboxFilter
  onClose: () => void
  /** Renaming this one; null saves the filter on screen as a new view. */
  saved: InboxSavedView | null
  suggestedName: string
}) {
  const setView = useSetInboxView()
  const [name, setName] = useState(saved?.name ?? suggestedName)
  const [density, setDensity] = useState<InboxDensity>(saved?.density ?? 'detailed')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim() || busy) return
    setBusy(true)
    setError(null)
    void setView(saved ? { inboxViewId: saved.id, name: name.trim(), density } : { name: name.trim(), filter, density })
      .then(() => {
        showToast({ tone: 'success', title: saved ? 'View renamed' : `Saved “${name.trim()}”` })
        onClose()
      })
      .catch((reason: unknown) => setError(describeError(reason).message))
      .finally(() => setBusy(false))
  }
  return (
    <Dialog
      description={saved ? undefined : 'A view keeps what the inbox shows now (which part, which kinds, which spaces) as a tab of its own.'}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} disabled={!name.trim()} type="submit" variant="primary">
            {saved ? 'Save' : 'Save view'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title={saved ? 'Rename view' : 'Save inbox view'}
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
      <TextField autoFocus label="Name" maxLength={40} onChange={(event) => setName(event.target.value)} value={name} />
      <SelectField label="Rows" onChange={(event) => setDensity(event.target.value === 'compact' ? 'compact' : 'detailed')} value={density}>
        <option value="detailed">Detailed</option>
        <option value="compact">Compact</option>
      </SelectField>
    </Dialog>
  )
}
