import { useDeferredValue, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { useTheme } from '../theme/themeContext'
import {
  Banner,
  Button,
  EmptyState,
  ErrorState,
  IconButton,
  Menu,
  PageHeader,
  PageSkeleton,
  SegmentedControl,
  StatusChip,
  TextField,
  formatFullDate,
  formatRelative,
  showToast,
} from '../ui'
import {
  duplicateWhiteboard,
  listWhiteboards,
  previewUrl,
  updateWhiteboard,
  whiteboardKeys,
  whiteboardPath,
  type Whiteboard,
} from './api'
import { NewWhiteboardDialog, RenameDialog } from './dialogs'
import { errorText } from './errors'
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
  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-page--wide">
        <PageHeader
          actions={
            <Button disabled={workspaces.length === 0} icon="plus" onClick={() => setCreating(true)} variant="primary">
              New whiteboard
            </Button>
          }
          icon="whiteboard"
          title="Whiteboards"
        >
          Sketch, plan and diagram together. Agents can read and draw on the same boards, and one board can sit on many tasks,
          projects and pages.
        </PageHeader>
        <div className="aw-boards__filters">
          <TextField
            aria-label="Search whiteboards"
            className="aw-boards__search"
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search titles and words on boards"
            type="search"
            value={query}
          />
          <SegmentedControl
            label="Show"
            onChange={setView}
            options={[
              { value: 'boards', label: 'Boards' },
              { value: 'trash', label: 'Trash', icon: 'trash' },
            ]}
            value={view}
          />
        </div>
        {boards.isPending ? <PageSkeleton cards={6} label="Loading whiteboards" /> : null}
        {boards.isError ? (
          <ErrorState action={<Button icon="retry" onClick={() => void boards.refetch()}>Try again</Button>} title="Whiteboards didn’t load">
            {errorText(boards.error, 'Check your connection, then try again.')}
          </ErrorState>
        ) : null}
        {boards.data && boards.data.whiteboards.length === 0 ? (
          search.trim() ? (
            <EmptyState icon="search" title="No boards match">
              Try other words, or clear the search.
            </EmptyState>
          ) : view === 'trash' ? (
            <EmptyState icon="trash" title="The trash is empty">
              Boards you move to the trash wait here, drawing and links intact, until you bring them back.
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
          <Banner tone="info">Showing the 200 most recently changed boards. Search to find older ones.</Banner>
        ) : null}
      </div>
      <NewWhiteboardDialog onClose={() => setCreating(false)} open={creating} workspaces={workspaces} />
    </AppShell>
  )
}

function BoardCard({ board }: { board: Whiteboard }) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const { preset } = useTheme()
  const [renaming, setRenaming] = useState(false)
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
      .catch((reason: unknown) => showToast({ tone: 'danger', title: errorText(reason, 'That didn’t work.') }))
  const duplicate = () =>
    duplicateWhiteboard(board.id)
      .then((copy) => {
        void refresh()
        navigate(whiteboardPath(copy.whiteboard.id))
      })
      .catch((reason: unknown) => showToast({ tone: 'danger', title: errorText(reason, 'The board wasn’t copied.') }))
  return (
    <li className="aw-boardcard">
      <Link aria-hidden="true" className="aw-boardcard__preview" tabIndex={-1} to={href}>
        {board.elementCount > 0 ? (
          <img alt="" loading="lazy" src={previewUrl(board.id, board.sceneSeq, preset === 'midnight')} />
        ) : (
          <span className="aw-boardcard__blank">Empty board</span>
        )}
      </Link>
      <div className="aw-boardcard__body">
        <div className="aw-boardcard__title">
          <Link to={href}>{board.title}</Link>
          <Menu
            items={
              board.deleted
                ? [{ label: 'Restore', icon: 'undo', onSelect: () => void trash(false) }]
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
              {board.linkCount === 1 ? '1 link' : `${board.linkCount} links`}
            </StatusChip>
          ) : null}
        </p>
        <p className="aw-boardcard__meta" title={formatFullDate(board.updatedAt)}>
          Changed {formatRelative(board.updatedAt)}
          {board.updatedBy ? ` by ${board.updatedBy}` : ''}
        </p>
      </div>
      <RenameDialog board={board} onClose={() => setRenaming(false)} open={renaming} />
    </li>
  )
}
