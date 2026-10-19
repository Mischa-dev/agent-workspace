import { useDeferredValue, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import {
  Banner,
  Button,
  Dialog,
  Icon,
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
  type Whiteboard,
} from '../whiteboards/api'
import { NewWhiteboardDialog } from '../whiteboards/dialogs'

const SHOWN = 5

const newestFirst = (left: Whiteboard, right: Whiteboard) => Date.parse(right.updatedAt) - Date.parse(left.updatedAt)

/**
 * The project's whiteboards: the ones that belong to it and the ones linked to
 * it from elsewhere, newest first. Linked ones can be unlinked; the project's
 * own stay until they're moved or put in the trash.
 */
export default function ProjectWhiteboards({ canEdit, projectId }: { canEdit: boolean; projectId: string }) {
  const queryClient = useQueryClient()
  const own = useQuery({
    queryKey: whiteboardKeys.list({ projectId }),
    queryFn: () => listWhiteboards({ projectId }),
  })
  const linked = useQuery({
    queryKey: whiteboardKeys.linked('project', projectId),
    queryFn: () => linkedWhiteboards('project', projectId),
  })
  const [dialog, setDialog] = useState<'new' | 'link' | null>(null)

  const boards = new Map<string, Whiteboard>()
  for (const board of [...(own.data?.whiteboards ?? []), ...(linked.data?.whiteboards ?? [])]) {
    if (!board.deleted) boards.set(board.id, board)
  }
  const list = [...boards.values()].sort(newestFirst)
  const refresh = () => queryClient.invalidateQueries({ queryKey: whiteboardKeys.all })

  const unlink = (board: Whiteboard) => {
    void unlinkWhiteboard(board.id, 'project', projectId)
      .then(() => {
        void refresh()
        showToast({
          title: `${board.title} unlinked`,
          onUndo: () => linkWhiteboard(board.id, 'project', projectId).then(refresh),
        })
      })
      .catch((reason: unknown) => showToast({ tone: 'danger', title: 'The whiteboard wasn’t unlinked', description: describeError(reason).message }))
  }

  return (
    <Section
      actions={
        canEdit ? (
          <div className="aw-overview__actions">
            <Button icon="link" onClick={() => setDialog('link')} size="sm">
              Link a whiteboard
            </Button>
            <Button icon="plus" onClick={() => setDialog('new')} size="sm">
              New whiteboard
            </Button>
          </div>
        ) : null
      }
      id="project-whiteboards"
      title="Whiteboards"
    >
      {own.isPending || linked.isPending ? <ListSkeleton label="Loading whiteboards" rows={1} /> : null}
      {own.isError || linked.isError ? (
        <Banner
          onRetry={() => {
            if (own.isError) void own.refetch()
            if (linked.isError) void linked.refetch()
          }}
          tone="danger"
        >
          Whiteboards didn’t load.
        </Banner>
      ) : null}
      {own.isSuccess && linked.isSuccess && list.length === 0 ? (
        <p className="aw-quiet">No whiteboards here yet. Sketch a plan or a diagram; agents can read and draw on it too.</p>
      ) : null}
      {list.length > 0 ? (
        <RowList label="Whiteboards">
          {list.slice(0, SHOWN).map((board) => {
            const fromElsewhere = board.projectId !== projectId
            return (
              <Row
                actions={
                  canEdit && fromElsewhere ? (
                    <IconButton icon="unlink" label={`Unlink ${board.title}`} onClick={() => unlink(board)} size="sm" />
                  ) : (
                    <Icon className="aw-row__chev" name="chevron-right" />
                  )
                }
                className="aw-row--link"
                key={board.id}
                leading={<Icon className="aw-row__glyph" name="whiteboard" />}
                meta={
                  <span title={formatFullDate(board.updatedAt)}>
                    {fromElsewhere ? `Linked from ${board.projectName ?? board.organizationName} · ` : ''}
                    Changed {formatRelative(board.updatedAt)}
                    {board.updatedBy ? ` by ${board.updatedBy}` : ''}
                  </span>
                }
                title={
                  <Link className="aw-row__link" to={whiteboardPath(board.id)}>
                    {board.title}
                  </Link>
                }
              />
            )
          })}
        </RowList>
      ) : null}
      {list.length > SHOWN ? (
        <p className="aw-overview__more">
          And {list.length - SHOWN} more on the <Link to="/whiteboards">Whiteboards</Link> page.
        </p>
      ) : null}
      <NewWhiteboardDialog
        link={{ targetKind: 'project', targetId: projectId }}
        onClose={() => setDialog(null)}
        open={dialog === 'new'}
        projectId={projectId}
      />
      <LinkWhiteboardDialog
        exclude={new Set(list.map((board) => board.id))}
        onClose={() => setDialog(null)}
        open={dialog === 'link'}
        projectId={projectId}
      />
    </Section>
  )
}

/** Picks a whiteboard from elsewhere in the workspace to show in this project. */
function LinkWhiteboardDialog({
  exclude,
  onClose,
  open,
  projectId,
}: {
  exclude: ReadonlySet<string>
  onClose: () => void
  open: boolean
  projectId: string
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
  const link = (board: Whiteboard) => {
    setBusy(board.id)
    setError(null)
    void linkWhiteboard(board.id, 'project', projectId)
      .then(() => {
        void queryClient.invalidateQueries({ queryKey: whiteboardKeys.all })
        showToast({ tone: 'success', title: `${board.title} linked` })
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
      {boards.isError ? <Banner onRetry={() => void boards.refetch()} tone="danger">Whiteboards didn’t load.</Banner> : null}
      {boards.data && candidates.length === 0 ? (
        <p className="aw-quiet">{search.trim() ? 'No other whiteboards match.' : 'Every whiteboard you can see is already here.'}</p>
      ) : null}
      {candidates.length ? (
        <RowList label="Whiteboards to link">
          {candidates.map((board) => (
            <Row
              actions={
                <Button busy={busy === board.id} disabled={busy !== null} onClick={() => link(board)} size="sm">
                  Link
                </Button>
              }
              key={board.id}
              meta={board.projectName ?? board.organizationName}
              title={board.title}
            />
          ))}
        </RowList>
      ) : null}
      {error ? <Banner tone="danger">{error}</Banner> : null}
    </Dialog>
  )
}
