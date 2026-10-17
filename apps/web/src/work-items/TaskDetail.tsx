import { lazy, Suspense, useState, type ReactNode } from 'react'
import { Link } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { formatFullDate } from '../conversations/timeFormat'
import { LaunchCard } from '../launch/LaunchCard'
import { Banner, Button, ErrorState, IconButton, MessageSkeleton, StatusChip } from '../ui'
import { useSetWorkItemArchived, useWorkItem, useWorkItemWorkflow } from './hooks'
import { LabelManager } from './LabelManager'
import { workItemHref } from './links'
import { useReleaseClaim } from './planningHooks'
import { TaskActivity } from './TaskActivity'
import { TaskFiles } from './TaskFiles'
import { TaskDependencies, TaskSubtasks, type OpenTask } from './TaskConnections'
import { AcceptanceCriteria } from './TaskCriteria'
import { DescriptionField, TitleField } from './TaskFields'
import { TaskLinks } from './TaskLinks'
import { TaskProperties } from './TaskProperties'
import { OpenInCodex } from './TaskRunActions'
import { editErrorMessage } from './taskEdits'
import type { WorkItem, WorkItemWorkflow } from './types'
import { useTaskSave } from './useTaskSave'

export type TaskLayout = 'panel' | 'page'

// Readiness, AI suggestions and summaries load in their own chunk.
const TaskAiPanel = lazy(() => import('../ai/TaskAiPanel'))
// The boards on this task load in the whiteboards' own small chunk.
const LinkedWhiteboards = lazy(() =>
  import('../whiteboards/LinkedWhiteboards').then((module) => ({ default: module.LinkedWhiteboards })),
)

/**
 * A task, loaded by id: the side panel on the list and board, and the body of
 * the full page. Every field saves on its own as it changes.
 */
export function TaskDetail({
  layout,
  onClose,
  onOpenTask,
  projectArchived,
  projectId,
  taskId,
}: {
  layout: TaskLayout
  onClose?: () => void
  onOpenTask: OpenTask
  projectArchived: boolean
  projectId: string
  taskId: string
}) {
  const item = useWorkItem(projectId, taskId)
  const workflow = useWorkItemWorkflow(projectId)
  if (item.isPending || workflow.isPending) {
    return (
      <div className={`aw-taskdetail aw-taskdetail--${layout}`}>
        {onClose ? <PanelBar onClose={onClose} /> : null}
        <MessageSkeleton label="Loading work item" rows={3} />
      </div>
    )
  }
  if (item.isError || workflow.isError) {
    return (
      <div className={`aw-taskdetail aw-taskdetail--${layout}`}>
        {onClose ? <PanelBar onClose={onClose} /> : null}
        <ErrorState
          action={<Button onClick={() => void Promise.all([item.refetch(), workflow.refetch()])}>Try again</Button>}
          title="This work item isn’t available"
        >
          It may have been removed, or your access may have changed. If your connection dropped, try again.
        </ErrorState>
      </div>
    )
  }
  return (
    <TaskDetailBody
      item={item.data}
      key={item.data.id}
      layout={layout}
      onClose={onClose}
      onOpenTask={onOpenTask}
      projectArchived={projectArchived}
      workflow={workflow.data}
    />
  )
}

function PanelBar({ children, onClose }: { children?: ReactNode; onClose: () => void }) {
  return (
    <div className="aw-taskdetail__bar">
      {children}
      <span className="aw-taskdetail__spacer" />
      <IconButton icon="x" label="Close panel" onClick={onClose} shortcut="Esc" side="bottom" />
    </div>
  )
}

function TaskDetailBody({
  item,
  layout,
  onClose,
  onOpenTask,
  projectArchived,
  workflow,
}: {
  item: WorkItem
  layout: TaskLayout
  onClose?: () => void
  onOpenTask: OpenTask
  projectArchived: boolean
  workflow: WorkItemWorkflow
}) {
  const save = useTaskSave(item)
  const disabled = projectArchived || Boolean(item.archivedAt) || !item.capabilities.edit
  const [managingLabels, setManagingLabels] = useState(false)

  const head = (
    <div className="aw-workitem__head">
      {layout === 'panel' && onClose ? (
        <PanelBar onClose={onClose}>
          <span className="aw-workitem__key">{item.key}</span>
          {item.archivedAt ? <StatusChip icon="archive">Archived</StatusChip> : null}
          <Link className="aw-taskdetail__full" to={workItemHref(item.projectId, item.id)}>
            Open full page
          </Link>
        </PanelBar>
      ) : (
        <div className="aw-workitem__top">
          <span className="aw-workitem__key">{item.key}</span>
          {item.archivedAt ? <StatusChip icon="archive">Archived</StatusChip> : null}
        </div>
      )}
      <h1 className="visually-hidden" id={layout === 'page' ? 'workspace-heading' : undefined}>
        {item.key}: {item.title}
      </h1>
      <TitleField disabled={disabled} onSave={(title) => save.save('Title', { title })} value={item.title} />
      <DescriptionField disabled={disabled} onSave={(description) => save.save('Description', { description })} value={item.description} />
      <ClaimStatus item={item} projectArchived={projectArchived} />
      {item.archivedAt ? (
        <Banner icon="archive" title="This work item is archived.">
          It’s hidden from lists and can’t change until it’s restored.
        </Banner>
      ) : null}
      {save.problem ? (
        <Banner
          action={<IconButton icon="x" label="Dismiss" onClick={save.clearProblem} size="sm" tooltip={false} />}
          role="alert"
          title={save.problem.conflict ? 'A newer version exists.' : 'That didn’t save.'}
          tone="warning"
        >
          {save.problem.message}
        </Banner>
      ) : null}
      <p aria-live="polite" className="aw-props__status">
        {save.saving ? `Saving ${save.saving.toLocaleLowerCase()}…` : ''}
      </p>
    </div>
  )

  const side = (
    <aside aria-label="Work item details" className="aw-workitem__side">
      <TaskProperties
        disabled={disabled}
        item={item}
        onManageLabels={() => setManagingLabels(true)}
        onOpenTask={onOpenTask}
        projectArchived={projectArchived}
        save={save}
        workflow={workflow}
      />
      <Suspense fallback={null}>
        <TaskAiPanel disabled={disabled} projectId={item.projectId} taskId={item.id} version={item.version} />
      </Suspense>
      <OpenInCodex item={item} projectArchived={projectArchived} />
      {!disabled ? <LaunchCard item={item} /> : null}
      <ArchiveControl item={item} projectArchived={projectArchived} />
    </aside>
  )

  return (
    <div className={`aw-workitem aw-taskdetail aw-taskdetail--${layout}`}>
      {head}
      {side}
      <div className="aw-workitem__rest">
        <TaskSubtasks disabled={disabled} item={item} onOpen={onOpenTask} />
        <TaskDependencies disabled={disabled} item={item} onOpen={onOpenTask} save={save} />
        <AcceptanceCriteria disabled={disabled} item={item} />
        <TaskLinks disabled={disabled} item={item} />
        <TaskFiles disabled={disabled} item={item} />
        <Suspense fallback={null}>
          <LinkedWhiteboards canEdit={!disabled} projectId={item.projectId} targetId={item.id} targetKind="task" />
        </Suspense>
        <TaskActivity disabled={disabled} item={item} />
      </div>
      {managingLabels ? <LabelManager onClose={() => setManagingLabels(false)} projectId={item.projectId} /> : null}
    </div>
  )
}

/** Who is working on this right now. Agents claim a task before starting it; the claim ends on its own. */
function ClaimStatus({ item, projectArchived }: { item: WorkItem; projectArchived: boolean }) {
  const session = useAuthenticatedSession()
  const release = useReleaseClaim()
  const claim = item.claim
  if (!claim) return null
  // Releasing is a task write: it needs edit rights and a project that isn't archived.
  const canRelease =
    !projectArchived && item.capabilities.edit && (item.capabilities.archive || claim.actor.id === session.user.actorId)
  return (
    <Banner
      action={
        canRelease ? (
          <Button
            busy={release.isPending}
            onClick={() => release.mutate({ projectId: item.projectId, workItemId: item.id, idempotencyKey: crypto.randomUUID() })}
            size="sm"
          >
            Release
          </Button>
        ) : undefined
      }
      icon={claim.actor.kind === 'external_agent' ? 'bot' : 'users'}
      title={`${claim.actor.displayName} is working on this.`}
      tone="agent"
    >
      {claim.note ? `${claim.note} · ` : ''}Claimed until {formatFullDate(claim.expiresAt)}.
      {release.isError ? ` ${editErrorMessage(release.error, 'Releasing the claim')}` : ''}
    </Banner>
  )
}

function ArchiveControl({ item, projectArchived }: { item: WorkItem; projectArchived: boolean }) {
  const archive = useSetWorkItemArchived()
  if (!item.capabilities.archive || projectArchived) return null
  return (
    <div className="aw-sidecard aw-sidecard--quiet">
      <Button
        busy={archive.isPending}
        icon={item.archivedAt ? 'undo' : 'archive'}
        onClick={() =>
          archive.mutate({
            projectId: item.projectId,
            workItemId: item.id,
            expectedVersion: item.version,
            archived: !item.archivedAt,
            idempotencyKey: crypto.randomUUID(),
          })
        }
        size="sm"
        variant="ghost"
      >
        {item.archivedAt ? 'Restore work item' : 'Archive work item'}
      </Button>
      {archive.isError ? <Banner tone="danger">{editErrorMessage(archive.error, 'Archiving')}</Banner> : null}
    </div>
  )
}
