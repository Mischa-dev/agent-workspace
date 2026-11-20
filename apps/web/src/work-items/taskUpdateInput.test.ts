import { describe, expect, it } from 'vitest'

import { taskUpdateInput } from './api'

describe('taskUpdateInput', () => {
  it('turns a cleared value into the operation’s clear, never into a missing field', () => {
    expect(
      taskUpdateInput('task-1', {
        expectedVersion: 4,
        assigneeActorId: null,
        parentWorkItemId: null,
        milestoneId: null,
        dueDate: null,
        estimate: null,
      }),
    ).toEqual({ taskId: 'task-1', expectedVersion: 4, clearAssignee: true, clearParent: true, milestone: 'none', dueDate: '', clearEstimate: true })
  })

  it('leaves what the operation can’t carry yet to the old route', () => {
    expect(taskUpdateInput('task-1', { expectedVersion: 4, statusId: 'status-done' })).toBeNull()
    expect(taskUpdateInput('task-1', { expectedVersion: 4, fieldValues: { risk: null } })).toBeNull()
    expect(taskUpdateInput('task-1', { expectedVersion: 4, blockedReason: null })).toBeNull()
  })
})
