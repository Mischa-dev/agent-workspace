import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate, useParams } from 'react-router-dom'

import type { DashboardEntry, TileEntry, TileInput } from '../api/generated/operations'
import { opQuery, useOp } from '../api/ops'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { useShellPage } from '../shell/shellStore'
import { Button, ConfirmDialog, EmptyState, ErrorState, IconButton, Menu, PageHeader, PageSkeleton, describeError, showToast, type MenuEntry } from '../ui'
import { DashboardDetailsDialog, TileDialog } from './DashboardDialogs'
import { DashboardTile } from './DashboardTile'
import { MOST_TILES, dashboardsHref, moveTile, stepHeight, stepWidth, tileInputs } from './dashboardModel'

/**
 * One dashboard (T1.32): its tiles on a twelve-column grid, each a view's
 * chart, count or first records, or a note. Every figure is read as the
 * person looking, so a tile over records they can't see says so.
 */
export function DashboardPage() {
  const { projectId = '', dashboardId = '' } = useParams()
  const project = useProject(projectId)
  const dashboard = useQuery({ ...opQuery('dashboard_get', { dashboard: dashboardId }), enabled: Boolean(dashboardId) })
  const list = useQuery({ ...opQuery('dashboard_list', { spaceId: projectId }), enabled: Boolean(projectId) })
  const navigate = useNavigate()
  const update = useOp('dashboard_update', { invalidates: ['dashboard_get', 'dashboard_list'] })
  const remove = useOp('dashboard_delete', { invalidates: ['dashboard_list'] })
  const [editing, setEditing] = useState<TileEntry | 'new' | null>(null)
  const [details, setDetails] = useState(false)
  const [deleting, setDeleting] = useState(false)

  useShellPage({ project: project.data })
  if (project.isPending) return <ProjectPageLoading label="Loading the dashboard" />
  if (project.isError) return <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />
  if (dashboard.isPending) {
    return (
      <div className="aw-page">
        <PageSkeleton label="Loading the dashboard" />
      </div>
    )
  }
  if (dashboard.isError) {
    return (
      <div className="aw-page">
        <ErrorState error={dashboard.error} onRetry={() => void dashboard.refetch()} title="The dashboard didn’t load" />
        <p>
          <Link to={dashboardsHref(projectId)}>Back to dashboards</Link>
        </p>
      </div>
    )
  }

  const current = dashboard.data
  const canShare = list.data?.canShare === true
  const canEdit = !project.data.archivedAt && (!current.shared || canShare)
  const save = async (change: { tiles?: TileInput[]; share?: boolean }, failed: string) => {
    try {
      await update.mutateAsync({ dashboard: current.id, expectedVersion: current.version, ...change })
    } catch (error) {
      showToast({ tone: 'danger', title: failed, description: describeError(error).message })
      void dashboard.refetch()
      throw error
    }
  }
  const saveTiles = (tiles: TileInput[], failed = 'The dashboard wasn’t saved') => save({ tiles }, failed).catch(() => undefined)
  const putTile = async (tile: TileInput) => {
    const tiles = tileInputs(current.tiles)
    const at = tile.id ? tiles.findIndex((entry) => entry.id === tile.id) : -1
    await save({ tiles: at === -1 ? [...tiles, tile] : tiles.map((entry, index) => (index === at ? tile : entry)) }, 'The tile wasn’t saved')
  }
  const tileMenu = (tile: TileEntry, index: number): MenuEntry[] => {
    const tiles = tileInputs(current.tiles)
    const resized = (change: Partial<TileInput>) => tiles.map((entry, at) => (at === index ? { ...entry, ...change } : entry))
    return [
      { label: 'Edit tile', icon: 'pencil', onSelect: () => setEditing(tile) },
      { separator: true },
      { label: 'Move earlier', icon: 'arrow-up', disabled: index === 0, onSelect: () => void saveTiles(moveTile(tiles, index, -1)) },
      { label: 'Move later', icon: 'arrow-down', disabled: index === tiles.length - 1, onSelect: () => void saveTiles(moveTile(tiles, index, 1)) },
      { separator: true },
      { label: 'Wider', disabled: tile.width >= 12, onSelect: () => void saveTiles(resized({ width: stepWidth(tile.width, 1) })) },
      { label: 'Narrower', disabled: tile.width <= 3, onSelect: () => void saveTiles(resized({ width: stepWidth(tile.width, -1) })) },
      { label: 'Taller', disabled: tile.height >= 4, onSelect: () => void saveTiles(resized({ height: stepHeight(tile.height, 1) })) },
      { label: 'Shorter', disabled: tile.height <= 1, onSelect: () => void saveTiles(resized({ height: stepHeight(tile.height, -1) })) },
      { separator: true },
      { label: 'Remove tile', icon: 'trash', danger: true, onSelect: () => void saveTiles(tiles.filter((_, at) => at !== index), 'The tile wasn’t removed') },
    ]
  }
  const pageMenu: MenuEntry[] = [
    { label: 'Rename', icon: 'pencil', onSelect: () => setDetails(true) },
    ...(canShare
      ? [
          current.shared
            ? { label: 'Make it yours only', icon: 'lock' as const, onSelect: () => void save({ share: false }, 'The dashboard wasn’t changed').catch(() => undefined) }
            : { label: 'Share with the project', icon: 'users' as const, onSelect: () => void save({ share: true }, 'The dashboard wasn’t shared').catch(() => undefined) },
        ]
      : []),
    { separator: true },
    { label: 'Delete dashboard', icon: 'trash', danger: true, onSelect: () => setDeleting(true) },
  ]
  const addTile =
    canEdit && current.tiles.length < MOST_TILES ? (
      <Button icon="plus" onClick={() => setEditing('new')} variant={current.tiles.length ? 'secondary' : 'primary'}>
        Add tile
      </Button>
    ) : null

  return (
    <div className="aw-page aw-dash">
      <PageHeader
        actions={
          canEdit ? (
            <>
              {addTile}
              <Menu items={pageMenu} label="Dashboard actions" trigger={<IconButton icon="more" label="Dashboard actions" />} />
            </>
          ) : null
        }
        icon="chart"
        title={current.name}
      >
        {[current.shared ? null : 'Only you see this dashboard.', current.description || null].filter(Boolean).join(' ') || null}
      </PageHeader>
      {current.tiles.length === 0 ? (
        <EmptyState action={addTile} icon="chart" title="No tiles yet">
          Add a tile to show one of the project’s views: a chart view as its chart, any other as how many records it finds and the first few. A tile can be a note, too.
        </EmptyState>
      ) : (
        <div className="aw-dash__grid">
          {current.tiles.map((tile, index) => (
            <DashboardTile
              key={tile.id}
              menu={canEdit ? <Menu items={tileMenu(tile, index)} label="Tile actions" placement="bottom-end" trigger={<IconButton icon="more" label={`Actions for ${tile.title || tile.figures?.viewName || 'this tile'}`} size="sm" />} /> : null}
              projectId={projectId}
              tile={tile}
            />
          ))}
        </div>
      )}
      {editing ? <TileDialog onClose={() => setEditing(null)} onSave={putTile} projectId={projectId} shared={current.shared} tile={editing === 'new' ? null : editing} /> : null}
      {details ? <DashboardDetailsDialog canShare={canShare} dashboard={current as DashboardEntry} onClose={() => setDetails(false)} onSaved={() => undefined} projectId={projectId} /> : null}
      <ConfirmDialog
        busy={remove.isPending}
        confirmLabel="Delete dashboard"
        error={remove.error ? describeError(remove.error).message : null}
        onClose={() => setDeleting(false)}
        onConfirm={() =>
          remove.mutate(
            { dashboard: current.id },
            {
              onSuccess: () => {
                showToast({ tone: 'success', title: `${current.name} deleted` })
                void navigate(dashboardsHref(projectId))
              },
            },
          )
        }
        open={deleting}
        title={`Delete ${current.name}?`}
      >
        {current.shared ? 'It goes for everyone in the project. The collections and views it shows stay as they are.' : 'The collections and views it shows stay as they are.'}
      </ConfirmDialog>
    </div>
  )
}
