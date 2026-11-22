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

  it('hands a task over and takes it back', () => {
    expect(taskUpdateInput('task-1', { expectedVersion: 4, delegateActorId: 'actor-2' })).toEqual({ taskId: 'task-1', expectedVersion: 4, delegate: 'actor-2' })
    expect(taskUpdateInput('task-1', { expectedVersion: 4, delegateActorId: null })).toEqual({ taskId: 'task-1', expectedVersion: 4, delegate: 'none' })
  })

  it('carries a status and field values, and leaves a blocker change to go on its own', () => {
    expect(taskUpdateInput('task-1', { expectedVersion: 4, statusId: 'status-done' })).toEqual({ taskId: 'task-1', expectedVersion: 4, status: 'status-done' })
    expect(taskUpdateInput('task-1', { expectedVersion: 4, fieldValues: { risk: null } })).toEqual({ taskId: 'task-1', expectedVersion: 4, fields: { risk: null } })
    expect(taskUpdateInput('task-1', { expectedVersion: 4, blockedReason: null })).toBeNull()
  })
})
