import { Suspense, lazy, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { useProjects } from '../projects/hooks'
import { Banner, Button, Dialog, EmptyState, IconButton, Row, RowList, SelectField, countLabel, describeError, formatFullDate, formatRelative } from '../ui'
import {
  linkWhiteboard,
  restoreRevision,
  unlinkWhiteboard,
  whiteboardKeys,
  type WhiteboardDetail,
  type WhiteboardLink,
} from './api'

const ReferencedIn = lazy(() => import('../modules/references/ReferencedIn'))

const KIND_LABEL: Record<WhiteboardLink['targetKind'], string> = {
  project: 'Project',
  work_item: 'Task',
  page: 'Page',
}

/** Where a whiteboard is linked, and a way to add it to a project. Tasks
 * and pages link it from their own pages. */
export function LinksDialog({
  canEdit,
  detail,
  onClose,
  open,
}: {
  canEdit: boolean
  detail: WhiteboardDetail
  onClose: () => void
  open: boolean
}) {
  const queryClient = useQueryClient()
  const projects = useProjects()
  const [project, setProject] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const { whiteboard, links } = detail
  const linkable = (projects.data?.projects ?? []).filter(
    (candidate) =>
      !candidate.archivedAt &&
      candidate.organizationId === whiteboard.organizationId &&
      !links.some((link) => link.targetKind === 'project' && link.targetId === candidate.id),
  )
  const chosen = project || linkable[0]?.id || ''
  const run = (key: string, action: () => Promise<unknown>) => {
    setBusy(key)
    setError(null)
    void action()
      .then(() => queryClient.invalidateQueries({ queryKey: whiteboardKeys.all }))
      .catch((reason: unknown) => setError(describeError(reason).message))
      .finally(() => setBusy(null))
  }
  return (
    <Dialog
      description="One whiteboard can sit on many tasks, projects and pages. Taking a link off never deletes the whiteboard."
      footer={<Button onClick={onClose}>Done</Button>}
      onClose={onClose}
      open={open}
      title="Links"
      width={560}
    >
      {links.length === 0 ? (
        <EmptyState icon="link" title="Not linked anywhere yet">
          Link it to a project here, or open a task or page and add it there.
        </EmptyState>
      ) : (
        <RowList label="Linked to">
          {links.map((link) => (
            <Row
              actions={
                canEdit ? (
                  <IconButton
                    disabled={busy !== null}
                    icon="unlink"
                    label={`Unlink from ${link.title}`}
                    onClick={() =>
                      run(`${link.targetKind}:${link.targetId}`, () =>
                        unlinkWhiteboard(whiteboard.id, link.targetKind, link.targetId),
                      )
                    }
                    size="sm"
                  />
                ) : null
              }
              key={`${link.targetKind}:${link.targetId}`}
              meta={
                <>
                  <span>{KIND_LABEL[link.targetKind]}</span>
                  {link.targetKind === 'project' ? null : <span>{link.projectName}</span>}
                </>
              }
              title={<Link to={link.url}>{link.key ? `${link.key} ${link.title}` : link.title}</Link>}
            />
          ))}
        </RowList>
      )}
      {canEdit && linkable.length ? (
        <div className="aw-inline-form">
          <SelectField label="Link to a project" onChange={(event) => setProject(event.target.value)} value={chosen}>
            {linkable.map((candidate) => (
              <option key={candidate.id} value={candidate.id}>
                {candidate.name}
              </option>
            ))}
          </SelectField>
          <Button
            busy={busy === 'add'}
            disabled={!chosen || busy !== null}
            icon="link"
            onClick={() => run('add', () => linkWhiteboard(whiteboard.id, 'project', chosen))}
          >
            Link
          </Button>
        </div>
      ) : null}
      {open ? (
        <Suspense fallback={null}>
          <ReferencedIn id={whiteboard.id} kind="whiteboard" />
        </Suspense>
      ) : null}
      {error ? <Banner tone="danger">{error}</Banner> : null}
    </Dialog>
  )
}

/** Saved versions of the drawing, to put one back. */
export function HistoryDialog({
  canEdit,
  detail,
  onClose,
  open,
}: {
  canEdit: boolean
  detail: WhiteboardDetail
  onClose: () => void
  open: boolean
}) {
  const queryClient = useQueryClient()
  const [busy, setBusy] = useState<number | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [restored, setRestored] = useState<number | null>(null)
  const restore = (revision: number) => {
    setBusy(revision)
    setError(null)
    void restoreRevision(detail.whiteboard.id, revision)
      .then(() => {
        setRestored(revision)
        void queryClient.invalidateQueries({ queryKey: whiteboardKeys.all })
      })
      .catch((reason: unknown) => setError(describeError(reason).message))
      .finally(() => setBusy(null))
  }
  return (
    <Dialog
      description="The whiteboard saves a version every few minutes while people draw, and before anything removes a lot. Restoring saves the current drawing first."
      footer={<Button onClick={onClose}>Close</Button>}
      onClose={onClose}
      open={open}
      title="History"
      width={560}
    >
      {detail.revisions.length === 0 ? (
        <EmptyState icon="clock" title="No saved versions yet">
          The first one is saved once the whiteboard changes.
        </EmptyState>
      ) : (
        <RowList label="Saved versions">
          {detail.revisions.map((revision) => (
            <Row
              actions={
                canEdit ? (
                  <Button
                    busy={busy === revision.revision}
                    disabled={busy !== null}
                    onClick={() => restore(revision.revision)}
                    size="sm"
                  >
                    Restore
                  </Button>
                ) : null
              }
              key={revision.revision}
              meta={
                <>
                  <span title={formatFullDate(revision.createdAt)}>{formatRelative(revision.createdAt)}</span>
                  {revision.author ? <span>{revision.author}</span> : null}
                  <span>{countLabel(revision.elementCount, 'element')}</span>
                </>
              }
              title={revision.note ?? `Version ${revision.revision}`}
            />
          ))}
        </RowList>
      )}
      {restored ? <Banner tone="success">Restored. Everyone with the whiteboard open sees it now.</Banner> : null}
      {error ? <Banner tone="danger">{error}</Banner> : null}
    </Dialog>
  )
}
