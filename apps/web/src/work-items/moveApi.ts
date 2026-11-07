import { requestJson } from '../api/client'
import type { FieldType, FieldValue } from '../fields/fieldModel'

/** Moving a task, with its subtasks, to another project (AWD-102). */

export type MoveIssueReason = 'no_matching_field' | 'option_missing' | 'person_not_in_target' | 'link_outside_move' | 'page_link'

export type MoveCandidate = {
  id: string
  name: string
  /** The original value fits this field as it is. */
  takesValue: boolean
  options?: string[]
}

export type MoveIssue = {
  id: string
  taskId: string
  taskKey: string
  fieldId: string
  field: string
  fieldType: FieldType
  linkTarget?: 'task' | 'page'
  reason: MoveIssueReason
  message: string
  /** The value as it reads on the original; only the mover sees it. */
  value: unknown
  candidates?: MoveCandidate[]
}

export type MovePlan = {
  targetProjectId: string
  targetProjectKey: string
  tasks: Array<{ id: string; key: string; title: string; parentId?: string }>
  carried?: Array<{ taskId: string; taskKey: string; field: string; targetField: string; value: unknown; followsMove?: boolean }>
  issues?: MoveIssue[]
  notes?: string[]
  archivedFieldValues?: number
  snapshot: string
}

export type MoveResolution =
  | { issue: string; action: 'leave' }
  | { issue: string; action: 'set'; targetField: string; value?: FieldValue }

export type MoveDone = {
  moved: Array<{ from: string; to: string; id: string; title: string; url: string }>
  notes?: string[]
  created: boolean
  replayed?: boolean
  leftBehind?: number
}

function movePath(projectId: string, taskId: string) {
  return `/api/v1/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(taskId)}/project-move`
}

/** What a move would do. Changes nothing; only the person asking sees it. */
export function previewMove(projectId: string, taskId: string, targetProjectId: string) {
  return requestJson<MovePlan>(`${movePath(projectId, taskId)}?targetProjectId=${encodeURIComponent(targetProjectId)}`)
}

/**
 * Moves the task with these choices, or nothing. Send the same
 * idempotencyKey to retry the same move; it returns the first result.
 */
export function moveTask(
  projectId: string,
  taskId: string,
  input: { targetProjectId: string; snapshot: string; resolutions: MoveResolution[]; leaveUnresolved: boolean },
  idempotencyKey: string,
) {
  return requestJson<MoveDone>(movePath(projectId, taskId), { method: 'POST', json: input, idempotencyKey })
}
