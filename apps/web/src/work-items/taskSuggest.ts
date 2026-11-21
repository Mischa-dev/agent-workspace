import type { OpInput } from '../api/ops'
import type { WorkItemPatch } from './types'

type Patch = Omit<WorkItemPatch, 'expectedVersion'>

/** The fields someone who only suggests can propose; the rest (delegate, milestone, parent, blocker) wait for someone who edits. */
const PROPOSABLE: ReadonlySet<keyof Patch> = new Set([
  'title',
  'description',
  'priority',
  'statusId',
  'typeId',
  'assigneeActorId',
  'addLabelIds',
  'removeLabelIds',
  'startDate',
  'dueDate',
  'estimate',
  'fieldValues',
])

/**
 * The panel's change as a `task_update` call, which keeps it as a proposal
 * for someone who edits the task (T1.41). Ids go as they are: the operation
 * takes a status, type, person, label or field by id as well as by name.
 * Null when the change is one only an editor can make.
 */
export function suggestedUpdate(taskId: string, version: number, patch: Patch): OpInput<'task_update'> | null {
  const keys = (Object.keys(patch) as (keyof Patch)[]).filter((key) => patch[key] !== undefined)
  if (!keys.length || keys.some((key) => !PROPOSABLE.has(key))) return null
  const input: OpInput<'task_update'> = { taskId, expectedVersion: version }
  if (patch.title !== undefined) input.title = patch.title
  if (patch.description !== undefined) input.description = patch.description
  if (patch.priority !== undefined) input.priority = patch.priority
  if (patch.statusId !== undefined) input.status = patch.statusId
  if (patch.typeId !== undefined) input.type = patch.typeId
  if (patch.assigneeActorId === null) input.clearAssignee = true
  else if (patch.assigneeActorId !== undefined) input.assignee = patch.assigneeActorId
  if (patch.addLabelIds?.length) input.addLabels = patch.addLabelIds
  if (patch.removeLabelIds?.length) input.removeLabels = patch.removeLabelIds
  if (patch.startDate !== undefined) input.startDate = patch.startDate ?? ''
  if (patch.dueDate !== undefined) input.dueDate = patch.dueDate ?? ''
  if (patch.estimate === null) input.clearEstimate = true
  else if (patch.estimate !== undefined) input.estimate = patch.estimate
  if (patch.fieldValues) input.fields = { ...patch.fieldValues }
  return input
}
