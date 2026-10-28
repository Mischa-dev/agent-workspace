import { useDeferredValue, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from 'react-router-dom'

import { toastError } from '../app/mutations'
import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { useTheme } from '../theme/themeContext'
import {
  Banner,
  Button,
  ConfirmDialog,
  EmptyState,
  ErrorState,
  IconButton,
  Menu,
  PageHeader,
  PageSkeleton,
  SegmentedControl,
  StatusChip,
  TextField,
  countLabel,
  describeError,
  formatFullDate,
  formatRelative,
  showToast,
} from '../ui'
import {
  deleteWhiteboardForever,
  duplicateWhiteboard,
  emptyWhiteboardTrash,
  isDarkPreset,
  listWhiteboards,
  previewUrl,
  updateWhiteboard,
  whiteboardKeys,
  whiteboardPath,
  type Whiteboard,
} from './api'
import { NewWhiteboardDialog, RenameDialog } from './dialogs'
import './whiteboards.css'

type View = 'boards' | 'trash'

export function WhiteboardsPage() {
  const session = useAuthenticatedSession()
  const [view, setView] = useState<View>('boards')
  const [query, setQuery] = useState('')
  const search = useDeferredValue(query)
  const [creating, setCreating] = useState(false)
  const filters = { q: search, deleted: view === 'trash' }
  const boards = useQuery({
    queryKey: whiteboardKeys.list(filters),
    queryFn: () => listWhiteboards(filters),
    placeholderData: (previous) => previous,
  })
  const workspaces = boards.data?.workspaces ?? []
  const queryClient = useQueryClient()
  const [emptying, setEmptying] = useState(false)
  const empty = useMutation({
    mutationFn: emptyWhiteboardTrash,
    onSuccess: ({ deleted }) => {
      setEmptying(false)
      void queryClient.invalidateQueries({ queryKey: whiteboardKeys.all })
      const shown = boards.data?.whiteboards.length ?? 0
      showToast({
        tone: 'success',
        title: `Deleted ${countLabel(deleted, 'whiteboard')} for good`,
        description: deleted < shown ? 'The rest are in projects where you can only look.' : undefined,
      })
    },
  })
  const trashed = view === 'trash' && !search.trim() && (boards.data?.whiteboards.length ?? 0) > 0
  return (
    <AppShell home mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page">
        <PageHeader
          actions={
            <Button disabled={workspaces.length === 0} icon="plus" onClick={() => setCreating(true)} variant="primary">
              New whiteboard
            </Button>
          }
          icon="whiteboard"
          title="Whiteboards"
        >
          Sketch, plan and diagram together. Agents can read and draw on the same whiteboards, and one whiteboard can sit on many
          tasks, projects and pages.
        </PageHeader>
        <div className="aw-boards__filters">
          <TextField
            aria-label="Search whiteboards"
            className="aw-boards__search"
            compact
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search titles and words on whiteboards"
            type="search"
            value={query}
          />
          <SegmentedControl
            label="Show"
            onChange={setView}
            options={[
              { value: 'boards', label: 'Whiteboards' },
              { value: 'trash', label: 'Trash', icon: 'trash' },
            ]}
            value={view}
          />
          {trashed ? (
            <Button icon="trash" onClick={() => setEmptying(true)} variant="danger">
              Empty trash
            </Button>
          ) : null}
        </div>
        {boards.isPending ? <PageSkeleton cards={6} label="Loading whiteboards" /> : null}
        {boards.isError ? <ErrorState error={boards.error} onRetry={() => void boards.refetch()} title="Whiteboards didn’t load" /> : null}
        {boards.data && boards.data.whiteboards.length === 0 ? (
          search.trim() ? (
            <EmptyState
              action={
                <Button onClick={() => setQuery('')} variant="secondary">
                  Clear the search
                </Button>
              }
              icon="search"
              title="No whiteboards match"
            >
              Try other words, or clear the search.
            </EmptyState>
          ) : view === 'trash' ? (
            <EmptyState
              action={
                <Button onClick={() => setView('boards')} variant="secondary">
                  Back to whiteboards
                </Button>
              }
              icon="trash"
              title="The trash is empty"
            >
              Whiteboards you move to the trash wait here, drawing and links intact, until you bring them back or delete them for good.
            </EmptyState>
          ) : (
            <EmptyState
              action={
                <Button icon="plus" onClick={() => setCreating(true)} variant="primary">
                  New whiteboard
                </Button>
              }
              icon="whiteboard"
              title="No whiteboards yet"
            >
              Start one for a plan, an architecture sketch or a retro. Link it to tasks and pages later.
            </EmptyState>
          )
        ) : null}
        {boards.data && boards.data.whiteboards.length > 0 ? (
          <ul aria-label={view === 'trash' ? 'Whiteboards in the trash' : 'Whiteboards'} className="aw-boards">
            {boards.data.whiteboards.map((board) => (
              <BoardCard board={board} key={board.id} />
            ))}
          </ul>
        ) : null}
        {boards.data && boards.data.whiteboards.length === 200 ? (
          <Banner tone="info">Showing the 200 most recently changed whiteboards. Search to find older ones.</Banner>
        ) : null}
      </div>
      <NewWhiteboardDialog onClose={() => setCreating(false)} open={creating} workspaces={workspaces} />
      <ConfirmDialog
        busy={empty.isPending}
        confirmLabel="Empty trash"
        error={empty.error ? describeError(empty.error).message : null}
        onClose={() => {
          setEmptying(false)
          empty.reset()
        }}
        onConfirm={() => empty.mutate()}
        open={emptying}
        title="Empty the trash?"
      >
        Every whiteboard in the trash is deleted for good, with its drawing, images and history. This can’t be undone.
      </ConfirmDialog>
    </AppShell>
  )
}

function BoardCard({ board }: { board: Whiteboard }) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const { preset } = useTheme()
  const [renaming, setRenaming] = useState(false)
  const [purging, setPurging] = useState(false)
  const purge = useMutation({
    mutationFn: () => deleteWhiteboardForever(board.id),
    onSuccess: () => {
      setPurging(false)
      void refresh()
      showToast({ tone: 'success', title: `Deleted “${board.title}” for good` })
    },
  })
  const href = whiteboardPath(board.id)
  const refresh = () => queryClient.invalidateQueries({ queryKey: whiteboardKeys.all })
  const trash = (deleted: boolean) =>
    updateWhiteboard(board.id, { deleted })
      .then(() => {
        void refresh()
        showToast({
          tone: 'success',
          title: deleted ? `Moved “${board.title}” to the trash` : `Restored “${board.title}”`,
          onUndo: deleted ? () => trash(false) : undefined,
        })
      })
      .catch((reason: unknown) =>
        toastError(deleted ? 'The whiteboard wasn’t moved to the trash' : 'The whiteboard wasn’t restored', reason),
      )
  const duplicate = () =>
    duplicateWhiteboard(board.id)
      .then((copy) => {
        void refresh()
        navigate(whiteboardPath(copy.whiteboard.id))
      })
      .catch((reason: unknown) => toastError('The whiteboard wasn’t copied', reason))
  return (
    <li className="aw-boardcard">
      <Link aria-hidden="true" className="aw-boardcard__preview" tabIndex={-1} to={href}>
        {board.elementCount > 0 ? (
          <img alt="" loading="lazy" src={previewUrl(board.id, board.sceneSeq, isDarkPreset(preset))} />
        ) : (
          <span className="aw-boardcard__blank">Nothing drawn yet</span>
        )}
      </Link>
      <div className="aw-boardcard__body">
        <div className="aw-boardcard__title">
          <Link to={href}>{board.title}</Link>
          <Menu
            items={
              board.deleted
                ? [
                    { label: 'Restore', icon: 'undo', onSelect: () => void trash(false) },
                    { separator: true },
                    { label: 'Delete forever', icon: 'trash', danger: true, onSelect: () => setPurging(true) },
                  ]
                : [
                    { label: 'Rename', icon: 'pencil', onSelect: () => setRenaming(true) },
                    { label: 'Duplicate', icon: 'copy', onSelect: () => void duplicate() },
                    { separator: true },
                    { label: 'Move to trash', icon: 'trash', danger: true, onSelect: () => void trash(true) },
                  ]
            }
            label={`Actions for ${board.title}`}
            placement="bottom-end"
            trigger={<IconButton icon="more" label={`Actions for ${board.title}`} size="sm" />}
          />
        </div>
        <p className="aw-boardcard__meta">
          <StatusChip icon={board.projectId ? 'lock' : 'users'} tone="muted">
            {board.projectName ?? board.organizationName}
          </StatusChip>
          {board.linkCount > 0 ? (
            <StatusChip icon="link" tone="muted">
              {countLabel(board.linkCount, 'link')}
            </StatusChip>
          ) : null}
        </p>
        <p className="aw-boardcard__meta" title={formatFullDate(board.updatedAt)}>
          Changed {formatRelative(board.updatedAt)}
          {board.updatedBy ? ` by ${board.updatedBy}` : ''}
        </p>
      </div>
      <RenameDialog board={board} onClose={() => setRenaming(false)} open={renaming} />
      <ConfirmDialog
        busy={purge.isPending}
        confirmLabel="Delete forever"
        error={purge.error ? describeError(purge.error).message : null}
        onClose={() => {
          setPurging(false)
          purge.reset()
        }}
        onConfirm={() => purge.mutate()}
        open={purging}
        title={`Delete “${board.title}” forever?`}
      >
        Its drawing, images and history go for good, and it comes off every task, project and page it’s on. Copies stay.
      </ConfirmDialog>
    </li>
  )
}
