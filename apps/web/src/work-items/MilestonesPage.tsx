import { useState, type FormEvent } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link, useParams } from 'react-router-dom'

import { opQuery, useOp } from '../api/ops'
import { useWorkspaceId } from '../app/workspace'
import { useProjectExperience } from '../experience/layout'

import { isApiError } from '../api/errors'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { RememberPlace } from '../projects/RememberPlace'
import { useShellPage } from '../shell/shellStore'
import { WorkTabs } from '../shell/ProjectTabs'
import { Banner, Button, Dialog, EmptyState, ErrorState, IconButton, ListSkeleton, PageHeader, SegmentedControl, SelectField, TextArea, TextField, countLabel, describeError, showToast } from '../ui'
import { DatePicker, formatDateLabel } from '../ui/pickers'
import { formatEstimate } from './historyFormat'
import { useWorkItemWorkflow } from './hooks'
import { workItemsHref } from './links'
import { useCreateMilestone, useInvalidateWorkItems, useMilestoneList, useUpdateMilestone } from './planningHooks'
import { TaskGlyph } from './TaskGlyph'
import type { EstimateUnit, Milestone } from './types'
import './work.css'
import '../styles/work.css'

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
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  const milestones = useMilestoneList(projectId, true)
  const update = useUpdateMilestone()
  const workflow = useWorkItemWorkflow(projectId)
  const invalidate = useInvalidateWorkItems()
  const workspaceId = useWorkspaceId() ?? undefined
  const reorder = useOp('milestone_reorder', { workspaceId })
  const [shown, setShown] = useState<Shown>('open')
  const [editing, setEditing] = useState<Milestone | 'new' | null>(null)

  useShellPage({ project: project.data })
  if (project.isPending) {
    return (
      <ProjectPageLoading label="Loading milestones" />
    )
  }
  if (project.isError) {
    return (
      <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />
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
  // Open milestones come first in the project's order, so a row's place in
  // the open list is its place among all of them.
  const move = (milestone: Milestone, to: number) => {
    reorder.mutate(
      { milestoneId: milestone.id, position: to + 1, expectedVersion: milestone.version },
      {
        onError: (error) => showToast({ title: `${milestone.name} didn’t move`, description: saveMessage(error), tone: 'danger' }),
        onSettled: () => void invalidate(projectId),
      },
    )
  }
  const newMilestone = readOnly ? null : (
    <Button icon="plus" onClick={() => setEditing('new')} variant="primary">
      New milestone
    </Button>
  )

  return (
    <>
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
            {visible.map((milestone, index) => (
              <MilestoneRow
                busy={update.isPending && update.variables?.milestoneId === milestone.id}
                estimateUnit={workflow.data?.estimateUnit ?? 'points'}
                key={milestone.id}
                milestone={milestone}
                onArchive={(archived) => setArchived(milestone, archived)}
                onEdit={() => setEditing(milestone)}
                onMove={
                  milestone.archived || reorder.isPending
                    ? undefined
                    : {
                        up: index > 0 ? () => move(milestone, index - 1) : undefined,
                        down: index < visible.length - 1 ? () => move(milestone, index + 1) : undefined,
                      }
                }
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
    </>
  )
}

function MilestoneRow({
  busy,
  estimateUnit,
  milestone,
  onArchive,
  onEdit,
  onMove,
  projectId,
  readOnly,
}: {
  busy: boolean
  estimateUnit: EstimateUnit
  milestone: Milestone
  onArchive: (archived: boolean) => void
  onEdit: () => void
  onMove?: { up?: () => void; down?: () => void }
  projectId: string
  readOnly: boolean
}) {
  const lateTasks = Boolean(milestone.targetDate && milestone.latestDueDate && milestone.latestDueDate > milestone.targetDate)
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
          {milestone.estimateTotal > 0 ? (
            <span className="aw-milestone__date">
              {formatEstimate(milestone.estimateDone, estimateUnit)} of {formatEstimate(milestone.estimateTotal, estimateUnit)} done
            </span>
          ) : null}
          {milestone.latestDueDate ? (
            <span className={['aw-milestone__date', lateTasks ? 'is-late' : ''].filter(Boolean).join(' ')}>
              Last task due {formatDateLabel(milestone.latestDueDate)}
              {lateTasks ? ', after the target date' : ''}
            </span>
          ) : null}
          {milestone.initiative ? <span className="aw-milestone__date">Part of {milestone.initiative}</span> : null}
          {milestone.total ? (
            <Link className="aw-milestone__tasks" to={href}>
              {milestone.open ? `${countLabel(milestone.open, 'open task')}` : 'View tasks'}
            </Link>
          ) : null}
        </div>
      </div>
      {readOnly ? null : (
        <div className="aw-milestone__actions">
          {onMove ? (
            <>
              <IconButton disabled={!onMove.up} icon="arrow-up" label={`Move ${milestone.name} up`} onClick={onMove.up} size="sm" />
              <IconButton disabled={!onMove.down} icon="arrow-down" label={`Move ${milestone.name} down`} onClick={onMove.down} size="sm" />
            </>
          ) : null}
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
  // An existing milestone can join one of the workspace's initiatives.
  const workspaceId = useWorkspaceId() ?? undefined
  const showsInitiatives = useProjectExperience(projectId).shows('workspace.initiatives')
  const initiatives = useQuery({
    ...opQuery('initiative_list', {}, { workspaceId }),
    enabled: Boolean(milestone) && showsInitiatives,
    staleTime: 60_000,
  })
  const [initiativeId, setInitiativeId] = useState(milestone?.initiativeId ?? '')
  const pending = create.isPending || update.isPending
  const error = create.error ?? update.error

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim() || pending) return
    const fields = { name: name.trim(), description: description.trim(), targetDate: targetDate || null }
    const done = { onSuccess: onClose }
    if (milestone) {
      const joined = initiativeId !== (milestone.initiativeId ?? '') ? { initiativeId: initiativeId || null } : {}
      update.mutate(
        { projectId, milestoneId: milestone.id, expectedVersion: milestone.version, ...fields, ...joined, idempotencyKey: crypto.randomUUID() },
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
      dirty={
        name !== (milestone?.name ?? '') ||
        description !== (milestone?.description ?? '') ||
        targetDate !== (milestone?.targetDate ?? '') ||
        initiativeId !== (milestone?.initiativeId ?? '')
      }
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
      {milestone && showsInitiatives && (initiatives.data?.initiatives.length || milestone.initiativeId) ? (
        <SelectField
          hint="Work that spans projects. Its progress counts this milestone’s tasks."
          label="Initiative"
          onChange={(event) => setInitiativeId(event.target.value)}
          value={initiativeId}
        >
          <option value="">None</option>
          {milestone.initiativeId && !initiatives.data?.initiatives.some((entry) => entry.id === milestone.initiativeId) ? (
            <option value={milestone.initiativeId}>{milestone.initiative ?? 'Its initiative'}</option>
          ) : null}
          {(initiatives.data?.initiatives ?? [])
            .filter((entry) => !entry.archived)
            .map((entry) => (
              <option key={entry.id} value={entry.id}>
                {entry.title}
              </option>
            ))}
        </SelectField>
      ) : null}
      {error ? <Banner tone="danger">{saveMessage(error)}</Banner> : null}
    </Dialog>
  )
}
