import { Button } from '../ui'
import { priorityLabel } from './constants'
import { formatEstimate, formatIsoDate } from './historyFormat'
import { TaskNotice } from './TaskNotice'
import type { WorkItem } from './types'
import { FIELD_NAMES, fieldValue, type TaskField, type TaskSave } from './useTaskSave'

/**
 * Saves that stopped, under the title: someone else changed the same thing
 * first, or the save failed. Each says what happened and what the person can
 * do about it; nothing they typed is lost meanwhile.
 */
export function TaskSaveNotices({ item, save }: { item: WorkItem; save: TaskSave }) {
  return (
    <>
      <TitleNotice item={item} save={save} />
      {save.stopped.map((stop) => (
        <FieldNotice field={stop.field} key={stop.field} save={save} state={stop.state} theirs={stop.theirs ?? item} message={stop.message} />
      ))}
      {save.problem ? (
        <TaskNotice
          actions={
            <Button onClick={save.clearProblem} size="sm" variant="ghost">
              Dismiss
            </Button>
          }
          icon={save.problem.conflict ? 'triangle-alert' : 'circle-alert'}
          live="alert"
          title={save.problem.conflict ? 'Someone else changed this task first.' : `The ${save.problem.field} didn’t change.`}
          tone={save.problem.conflict ? 'warning' : 'danger'}
        >
          {save.problem.message}
        </TaskNotice>
      ) : null}
    </>
  )
}

/** A field as a person reads it: "Done", "Mara", "Oct 3, 2026". Empty is "". */
function readable(task: WorkItem, field: TaskField): string {
  switch (field) {
    case 'status':
      return task.status.name
    case 'priority':
      return task.priority === 'no_priority' ? '' : priorityLabel(task.priority)
    case 'assignee':
      return task.assignee?.displayName ?? ''
    case 'type':
      return task.itemType.name
    case 'milestone':
      return task.milestone?.name ?? ''
    case 'parent':
      return task.parent?.key ?? ''
    case 'startDate':
      return task.startDate ? formatIsoDate(task.startDate) : ''
    case 'dueDate':
      return task.dueDate ? formatIsoDate(task.dueDate) : ''
    case 'estimate':
      return task.estimate === null ? '' : formatEstimate(task.estimate, task.estimateUnit)
    default:
      return fieldValue(task, field)
  }
}

/** A picked field that didn't save: someone else changed it first, or the save failed. */
function FieldNotice({
  field,
  message,
  save,
  state,
  theirs,
}: {
  field: TaskField
  message: string | null
  save: TaskSave
  state: 'conflict' | 'failed'
  theirs: WorkItem
}) {
  const name = FIELD_NAMES[field]
  if (state === 'failed') {
    return (
      <TaskNotice
        actions={
          <>
            <Button onClick={() => save.retry(field)} size="sm" variant="primary">
              Try again
            </Button>
            <Button onClick={() => save.discard(field)} size="sm" variant="ghost">
              Dismiss
            </Button>
          </>
        }
        icon="circle-alert"
        live="alert"
        title={`The ${name} didn’t save.`}
        tone="danger"
      >
        {message}
      </TaskNotice>
    )
  }
  const value = readable(theirs, field)
  const title =
    field === 'blocked'
      ? theirs.blocked
        ? 'Someone else marked this blocked first.'
        : 'Someone else unblocked this first.'
      : value
        ? `Someone else set the ${name} to ${value} first.`
        : `Someone else cleared the ${name} first.`
  return (
    <TaskNotice
      actions={
        <>
          <Button onClick={() => save.keepMine(field)} size="sm" variant="primary">
            Keep yours
          </Button>
          <Button onClick={() => save.discard(field)} size="sm">
            Use theirs
          </Button>
        </>
      }
      icon="triangle-alert"
      live="alert"
      title={title}
      tone="warning"
    >
      Yours didn’t save. Keep yours to change it anyway, or use theirs.
    </TaskNotice>
  )
}

/** A title that didn't save. The typed title stays in the field until it's kept or let go. */
function TitleNotice({ item, save }: { item: WorkItem; save: TaskSave }) {
  const pending = save.pending('title')
  if (!pending || pending.state === 'saving') return null
  if (pending.state === 'failed') {
    return (
      <TaskNotice
        actions={
          <>
            <Button onClick={() => save.retry('title')} size="sm" variant="primary">
              Try again
            </Button>
            <Button onClick={() => save.discard('title')} size="sm" variant="ghost">
              Discard
            </Button>
          </>
        }
        icon="circle-alert"
        live="alert"
        title="Your title didn’t save."
        tone="danger"
      >
        {pending.message}
      </TaskNotice>
    )
  }
  return (
    <TaskNotice
      actions={
        <>
          <Button onClick={() => save.keepMine('title')} size="sm" variant="primary">
            Keep yours
          </Button>
          <Button onClick={() => save.discard('title')} size="sm">
            Use theirs
          </Button>
        </>
      }
      icon="triangle-alert"
      live="alert"
      title="Someone else changed the title while you were editing."
      tone="warning"
    >
      Theirs is “{(pending.theirs ?? item).title}”. Yours is still in the field above.
    </TaskNotice>
  )
}
