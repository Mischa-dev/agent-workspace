import { useDeferredValue, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { toastError } from '../app/mutations'
import {
  Banner,
  Button,
  Dialog,
  IconButton,
  ListSkeleton,
  Row,
  RowList,
  Section,
  TextField,
  describeError,
  formatFullDate,
  formatRelative,
  showToast,
} from '../ui'
import {
  linkWhiteboard,
  linkedWhiteboards,
  listWhiteboards,
  unlinkWhiteboard,
  whiteboardKeys,
  whiteboardPath,
  type LinkTargetKind,
  type Whiteboard,
} from './api'
import { NewWhiteboardDialog } from './dialogs'

const SERVER_KIND: Record<LinkTargetKind, string> = { task: 'work_item', project: 'project', page: 'page' }

/** The whiteboards on a task, project or page, with ways to add one. */
export function LinkedWhiteboards({
  canEdit,
  projectId,
  targetId,
  targetKind,
}: {
  canEdit: boolean
  projectId: string
  targetId: string
  targetKind: LinkTargetKind
}) {
  const queryClient = useQueryClient()
  const boards = useQuery({
    queryKey: whiteboardKeys.linked(targetKind, targetId),
    queryFn: () => linkedWhiteboards(targetKind, targetId),
  })
  const [dialog, setDialog] = useState<'new' | 'link' | null>(null)
  const refresh = () => queryClient.invalidateQueries({ queryKey: whiteboardKeys.all })
  // Taking a link off is undone from the toast; the whiteboard itself stays.
  const unlink = (board: Whiteboard) => {
    void unlinkWhiteboard(board.id, SERVER_KIND[targetKind], targetId)
      .then(() => {
        void refresh()
        showToast({
          title: `${board.title} unlinked`,
          onUndo: () =>
            void linkWhiteboard(board.id, targetKind, targetId)
              .then(refresh)
              .catch((reason: unknown) => toastError('It wasn’t linked again', reason)),
        })
      })
      .catch((reason: unknown) => toastError('The whiteboard wasn’t unlinked', reason))
  }
  const list = boards.data?.whiteboards ?? []
  return (
    <Section
      actions={
        canEdit ? (
          <span className="aw-linked-boards__actions">
            <Button icon="link" onClick={() => setDialog('link')} size="sm">
              Link a whiteboard
            </Button>
            <Button icon="plus" onClick={() => setDialog('new')} size="sm">
              New whiteboard
            </Button>
          </span>
        ) : null
      }
      id={`whiteboards-${targetId}`}
      title="Whiteboards"
    >
      {boards.isPending ? <ListSkeleton label="Loading whiteboards" rows={1} /> : null}
      {boards.isError ? (
        <Banner onRetry={() => void boards.refetch()} tone="danger" title="Whiteboards didn’t load.">
          {describeError(boards.error).message}
        </Banner>
      ) : null}
      {boards.data && list.length === 0 ? <p className="aw-linked-boards__empty">No whiteboards here yet.</p> : null}
      {list.length ? (
        <RowList label="Linked whiteboards">
          {list.map((board) => (
            <Row
              actions={
                canEdit ? (
                  <IconButton icon="unlink" label={`Unlink ${board.title}`} onClick={() => unlink(board)} size="sm" />
                ) : null
              }
              key={board.id}
              meta={
                <span>
                  Changed{' '}
                  <time dateTime={board.updatedAt} title={formatFullDate(board.updatedAt)}>
                    {formatRelative(board.updatedAt)}
                  </time>
                  {board.updatedBy ? ` by ${board.updatedBy}` : ''}
                </span>
              }
              title={<Link to={whiteboardPath(board.id)}>{board.title}</Link>}
            />
          ))}
        </RowList>
      ) : null}
      <NewWhiteboardDialog
        link={{ targetKind, targetId }}
        onClose={() => setDialog(null)}
        open={dialog === 'new'}
        projectId={projectId}
      />
      <LinkBoardDialog
        exclude={new Set(list.map((board) => board.id))}
        onClose={() => setDialog(null)}
        open={dialog === 'link'}
        targetId={targetId}
        targetKind={targetKind}
      />
    </Section>
  )
}

function LinkBoardDialog({
  exclude,
  onClose,
  open,
  targetId,
  targetKind,
}: {
  exclude: ReadonlySet<string>
  onClose: () => void
  open: boolean
  targetId: string
  targetKind: LinkTargetKind
}) {
  const queryClient = useQueryClient()
  const [query, setQuery] = useState('')
  const search = useDeferredValue(query)
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const boards = useQuery({
    queryKey: whiteboardKeys.list({ q: search, picker: true }),
    queryFn: () => listWhiteboards({ q: search }),
    enabled: open,
    placeholderData: (previous) => previous,
  })
  const candidates = (boards.data?.whiteboards ?? []).filter((board) => !exclude.has(board.id)).slice(0, 20)
  const link = (id: string) => {
    setBusy(id)
    setError(null)
    void linkWhiteboard(id, targetKind, targetId)
      .then(() => {
        void queryClient.invalidateQueries({ queryKey: whiteboardKeys.all })
        onClose()
      })
      .catch((reason: unknown) => setError(describeError(reason).message))
      .finally(() => setBusy(null))
  }
  return (
    <Dialog
      footer={
        <Button onClick={onClose} variant="secondary">
          Cancel
        </Button>
      }
      onClose={onClose}
      open={open}
      title="Link a whiteboard"
      width={520}
    >
      <TextField
        aria-label="Search whiteboards"
        autoFocus
        onChange={(event) => setQuery(event.target.value)}
        placeholder="Search whiteboards"
        type="search"
        value={query}
      />
      {boards.isPending ? <ListSkeleton label="Loading whiteboards" rows={3} /> : null}
      {boards.isError ? (
        <Banner onRetry={() => void boards.refetch()} tone="danger" title="Whiteboards didn’t load.">
          {describeError(boards.error).message}
        </Banner>
      ) : null}
      {boards.data && candidates.length === 0 ? <p className="aw-linked-boards__empty">No other whiteboards match.</p> : null}
      {candidates.length ? (
        <RowList label="Whiteboards to link">
          {candidates.map((board) => (
            <Row
              actions={
                <Button busy={busy === board.id} disabled={busy !== null} onClick={() => link(board.id)} size="sm">
                  Link
                </Button>
              }
              key={board.id}
              meta={<span>{board.projectName ?? board.organizationName}</span>}
              title={board.title}
            />
          ))}
        </RowList>
      ) : null}
      {error ? <Banner tone="danger">{error}</Banner> : null}
    </Dialog>
  )
}
