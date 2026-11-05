import { useState, type FormEvent } from 'react'
import { Link, useParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { RememberPlace } from '../projects/RememberPlace'
import { AppShell } from '../shell/AppShell'
import { WorkTabs } from '../shell/ProjectTabs'
import { Banner, Button, Dialog, EmptyState, ErrorState, ListSkeleton, PageHeader, SegmentedControl, TextArea, TextField, countLabel, describeError, showToast } from '../ui'
import { DatePicker, formatDateLabel } from '../ui/pickers'
import { workItemsHref } from './links'
import { useCreateMilestone, useMilestoneList, useUpdateMilestone } from './planningHooks'
import { TaskGlyph } from './TaskGlyph'
import type { Milestone } from './types'
import './work.css'

type Shown = 'open' | 'archived'

function milestoneHref(projectId: string, milestoneId: string) {
  return `${workItemsHref(projectId)}?milestoneId=${encodeURIComponent(milestoneId)}`
}

function saveMessage(error: unknown) {
  if (isApiError(error) && error.code === 'version_conflict') return 'Someone changed this milestone first. Close this and try again.'
  if (isApiError(error) && error.message) return error.message
  return `The milestone didn’t save. ${describeError(error).message}`
}

/** A project's milestones: how far along each is, when it's due, and its tasks one click away. */
export function MilestonesPage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  const milestones = useMilestoneList(projectId, true)
  const update = useUpdateMilestone()
  const [shown, setShown] = useState<Shown>('open')
  const [editing, setEditing] = useState<Milestone | 'new' | null>(null)

  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading label="Loading milestones" />
      </AppShell>
    )
  }
  if (project.isError) {
    return (
      <AppShell user={session.user}>
        <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }

  const readOnly = Boolean(project.data.archivedAt) || !milestones.data?.canEdit
  const all = milestones.data?.milestones ?? []
  const visible = all.filter((milestone) => (shown === 'archived' ? milestone.archived : !milestone.archived))
  const archivedCount = all.filter((milestone) => milestone.archived).length
  // Archiving is reversible, so it happens at once with Undo.
  const setArchived = (milestone: Milestone, archived: boolean) => {
    update.mutate(
      { projectId, milestoneId: milestone.id, expectedVersion: milestone.version, archived, idempotencyKey: crypto.randomUUID() },
      {
        onError: (error) =>
          showToast({ title: archived ? `${milestone.name} wasn’t archived` : `${milestone.name} wasn’t restored`, description: saveMessage(error), tone: 'danger' }),
        onSuccess: (saved) =>
          showToast({
            title: archived ? `Archived ${milestone.name}` : `Restored ${milestone.name}`,
            tone: 'success',
            onUndo: () =>
              update.mutateAsync({ projectId, milestoneId: milestone.id, expectedVersion: saved.version, archived: !archived, idempotencyKey: crypto.randomUUID() }),
          }),
      },
    )
  }
  const newMilestone = readOnly ? null : (
    <Button icon="plus" onClick={() => setEditing('new')} variant="primary">
      New milestone
    </Button>
  )

  return (
    <AppShell project={project.data} user={session.user}>
      <div className="aw-page aw-page--full aw-milestones">
        <PageHeader actions={newMilestone} icon="checklist" title="Milestones" />
        <WorkTabs current="milestones" projectId={project.data.id} />
        <RememberPlace location={{ kind: 'milestones' }} project={project.data} />
        {archivedCount ? (
          <SegmentedControl<Shown>
            label="Milestones to show"
            onChange={setShown}
            options={[
              { value: 'open', label: 'Open' },
              { value: 'archived', label: `Archived (${archivedCount})` },
            ]}
            value={shown}
          />
        ) : null}
        {milestones.isPending ? <ListSkeleton label="Loading milestones" rows={3} /> : null}
        {milestones.isError ? <ErrorState error={milestones.error} onRetry={() => void milestones.refetch()} title="Milestones didn’t load" /> : null}
        {milestones.isSuccess && visible.length === 0 ? (
          <EmptyState action={shown === 'archived' ? undefined : newMilestone} icon="checklist" title={shown === 'archived' ? 'No archived milestones' : 'No milestones yet'}>
            {shown === 'archived' ? 'Archived milestones show up here.' : 'A milestone gathers tasks toward a date, like a release.'}
          </EmptyState>
        ) : null}
        {visible.length ? (
          <ul aria-label="Milestones" className="aw-milestones__list">
            {visible.map((milestone) => (
              <MilestoneRow
                busy={update.isPending && update.variables?.milestoneId === milestone.id}
                key={milestone.id}
                milestone={milestone}
                onArchive={(archived) => setArchived(milestone, archived)}
                onEdit={() => setEditing(milestone)}
                projectId={projectId}
                readOnly={readOnly}
              />
            ))}
          </ul>
        ) : null}
      </div>
      {editing ? (
        <MilestoneDialog milestone={editing === 'new' ? null : editing} onClose={() => setEditing(null)} projectId={projectId} />
      ) : null}
    </AppShell>
  )
}

function MilestoneRow({
  busy,
  milestone,
  onArchive,
  onEdit,
  projectId,
  readOnly,
}: {
  busy: boolean
  milestone: Milestone
  onArchive: (archived: boolean) => void
  onEdit: () => void
  projectId: string
  readOnly: boolean
}) {
  const percent = Math.round(Math.min(1, Math.max(0, milestone.progress)) * 100)
  const href = milestoneHref(projectId, milestone.id)
  return (
    <li className={['aw-milestone', milestone.overdue ? 'is-overdue' : ''].filter(Boolean).join(' ')}>
      <div className="aw-milestone__main">
        <Link className="aw-milestone__name" to={href}>
          <TaskGlyph name="milestone" size={16} />
          <span className="aw-milestone__title">{milestone.name}</span>
        </Link>
        {milestone.description ? <p className="aw-milestone__description">{milestone.description}</p> : null}
        <div className="aw-milestone__meta">
          <span className="aw-progress">
            <progress aria-label={`${milestone.name} progress`} className="aw-progress__bar" max={100} value={percent} />
            <span className="aw-progress__text">
              {milestone.total ? `${milestone.finished} of ${milestone.total} done · ${percent}%` : 'No tasks yet'}
              {milestone.canceled ? ` · ${milestone.canceled} canceled` : ''}
            </span>
          </span>
          <span className="aw-milestone__date">
            <TaskGlyph name="calendar" size={14} />
            {milestone.targetDate ? (
              <time dateTime={milestone.targetDate} title={formatDateLabel(milestone.targetDate, { weekday: 'long', month: 'long', day: 'numeric', year: 'numeric' })}>
                {formatDateLabel(milestone.targetDate)}
              </time>
            ) : (
              'No target date'
            )}
            {milestone.overdue ? <strong className="aw-milestone__overdue"> Overdue</strong> : null}
          </span>
          {milestone.total ? (
            <Link className="aw-milestone__tasks" to={href}>
              {milestone.open ? `${countLabel(milestone.open, 'open task')}` : 'View tasks'}
            </Link>
          ) : null}
        </div>
      </div>
      {readOnly ? null : (
        <div className="aw-milestone__actions">
          <Button onClick={onEdit} size="sm" variant="ghost">
            Edit
          </Button>
          <Button busy={busy} onClick={() => onArchive(!milestone.archived)} size="sm" variant="ghost">
            {milestone.archived ? 'Restore' : 'Archive'}
          </Button>
        </div>
      )}
    </li>
  )
}

function MilestoneDialog({ milestone, onClose, projectId }: { milestone: Milestone | null; onClose: () => void; projectId: string }) {
  const create = useCreateMilestone()
  const update = useUpdateMilestone()
  const [name, setName] = useState(milestone?.name ?? '')
  const [description, setDescription] = useState(milestone?.description ?? '')
  const [targetDate, setTargetDate] = useState(milestone?.targetDate ?? '')
  const pending = create.isPending || update.isPending
  const error = create.error ?? update.error

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim() || pending) return
    const fields = { name: name.trim(), description: description.trim(), targetDate: targetDate || null }
    const done = { onSuccess: onClose }
    if (milestone) {
      update.mutate(
        { projectId, milestoneId: milestone.id, expectedVersion: milestone.version, ...fields, idempotencyKey: crypto.randomUUID() },
        done,
      )
    } else {
      create.mutate({ projectId, ...fields, idempotencyKey: crypto.randomUUID() }, done)
    }
  }

  return (
    <Dialog
      footer={
        <>
          <Button onClick={onClose} variant="secondary">
            Cancel
          </Button>
          <Button busy={pending} disabled={!name.trim()} type="submit" variant="primary">
            {milestone ? 'Save' : 'Create milestone'}
          </Button>
        </>
      }
      dirty={name !== (milestone?.name ?? '') || description !== (milestone?.description ?? '') || targetDate !== (milestone?.targetDate ?? '')}
      onClose={onClose}
      onSubmit={submit}
      open
      title={milestone ? 'Edit milestone' : 'New milestone'}
    >
      <TextField data-autofocus label="Name" maxLength={120} onChange={(event) => setName(event.target.value)} required value={name} />
      <div className="aw-field">
        <span aria-hidden="true" className="aw-field__label">
          Target date
        </span>
        <DatePicker className="aw-milestone__datefield" label="Target date" onChange={(value) => setTargetDate(value ?? '')} placeholder="No target date" value={targetDate || null} />
      </div>
      <TextArea label="Description" maxLength={2000} onChange={(event) => setDescription(event.target.value)} rows={3} value={description} />
      {error ? <Banner tone="danger">{saveMessage(error)}</Banner> : null}
    </Dialog>
  )
}
