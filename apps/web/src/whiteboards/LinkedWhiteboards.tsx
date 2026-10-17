import { useDeferredValue, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { Banner, Button, Dialog, IconButton, ListSkeleton, Row, RowList, Section, TextField, formatRelative } from '../ui'
import {
  linkWhiteboard,
  linkedWhiteboards,
  listWhiteboards,
  unlinkWhiteboard,
  whiteboardKeys,
  whiteboardPath,
  type LinkTargetKind,
} from './api'
import { NewWhiteboardDialog } from './dialogs'
import { errorText } from './errors'

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
  const [error, setError] = useState<string | null>(null)
  const unlink = (id: string) => {
    setError(null)
    void unlinkWhiteboard(id, SERVER_KIND[targetKind], targetId)
      .then(() => queryClient.invalidateQueries({ queryKey: whiteboardKeys.all }))
      .catch((reason: unknown) => setError(errorText(reason, 'The board wasn’t unlinked.')))
  }
  const list = boards.data?.whiteboards ?? []
  return (
    <Section
      actions={
        canEdit ? (
          <span className="aw-linked-boards__actions">
            <Button icon="link" onClick={() => setDialog('link')} size="sm">
              Link a board
            </Button>
            <Button icon="plus" onClick={() => setDialog('new')} size="sm">
              New board
            </Button>
          </span>
        ) : null
      }
      id={`whiteboards-${targetId}`}
      title="Whiteboards"
    >
      {boards.isPending ? <ListSkeleton label="Loading whiteboards" rows={1} /> : null}
      {boards.isError ? <Banner tone="danger">Whiteboards didn’t load.</Banner> : null}
      {boards.data && list.length === 0 ? <p className="aw-linked-boards__empty">No whiteboards here yet.</p> : null}
      {list.length ? (
        <RowList label="Linked whiteboards">
          {list.map((board) => (
            <Row
              actions={
                canEdit ? (
                  <IconButton icon="unlink" label={`Unlink ${board.title}`} onClick={() => unlink(board.id)} size="sm" />
                ) : null
              }
              key={board.id}
              meta={<span>Changed {formatRelative(board.updatedAt)}{board.updatedBy ? ` by ${board.updatedBy}` : ''}</span>}
              title={<Link to={whiteboardPath(board.id)}>{board.title}</Link>}
            />
          ))}
        </RowList>
      ) : null}
      {error ? <Banner tone="danger">{error}</Banner> : null}
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
      .catch((reason: unknown) => setError(errorText(reason, 'The board wasn’t linked.')))
      .finally(() => setBusy(null))
  }
  return (
    <Dialog footer={<Button onClick={onClose}>Cancel</Button>} onClose={onClose} open={open} title="Link a whiteboard" width={520}>
      <TextField
        aria-label="Search whiteboards"
        autoFocus
        onChange={(event) => setQuery(event.target.value)}
        placeholder="Search whiteboards"
        type="search"
        value={query}
      />
      {boards.isPending ? <ListSkeleton label="Loading whiteboards" rows={3} /> : null}
      {boards.data && candidates.length === 0 ? <p className="aw-linked-boards__empty">No other boards match.</p> : null}
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
