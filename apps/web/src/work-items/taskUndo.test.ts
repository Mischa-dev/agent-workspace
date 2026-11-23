import { describe, expect, it } from 'vitest'

import { inverseOf, stillAsSet } from './taskUndo'
import { EMPTY_PLANNING } from './testFixtures'
import type { WorkItem, WorkItemCard } from './types'

const urgent = { id: 'label-urgent', name: 'Urgent', color: 'red' }
const task: WorkItem = {
  id: 'task-a',
  projectId: 'project-a',
  key: 'AW-7',
  number: 7,
  title: 'Parse the flags',
  description: 'Old words',
  itemType: { id: 'type', name: 'Task' },
  status: { id: 'todo', name: 'Todo', semanticCategory: 'unstarted' },
  priority: 'medium',
  assignee: null,
  version: 1,
  archivedAt: null,
  createdAt: '',
  updatedAt: '',
  labels: [urgent],
  acceptanceCriteria: [],
  linkedThreads: [],
  linkedRepositories: [],
  linkedPullRequests: [],
  openIn: [],
  capabilities: { view: true, edit: true, archive: true },
  ...EMPTY_PLANNING,
}

describe('undoing a task change', () => {
  it('puts back exactly what the change replaced', () => {
    expect(inverseOf(task, { statusId: 'done', priority: 'high', title: 'New' })).toEqual({ statusId: 'todo', priority: 'medium', title: 'Parse the flags' })
    expect(inverseOf(task, { description: 'New words' })).toEqual({ description: 'Old words' })
  })

  it('takes back only the labels the change really added or removed', () => {
    // Urgent was already on, so adding it again changed nothing.
    expect(inverseOf(task, { addLabelIds: ['label-urgent', 'label-new'] })).toEqual({ removeLabelIds: ['label-new'] })
    // Removing one that wasn't on changed nothing either.
    expect(inverseOf(task, { removeLabelIds: ['label-urgent', 'label-missing'] })).toEqual({ addLabelIds: ['label-urgent'] })
  })

  it('won’t guess what a card doesn’t show', () => {
    const card = { ...task } as unknown as WorkItemCard & { description?: string; delegate?: unknown }
    delete card.description
    delete card.delegate
    expect(inverseOf(card, { description: 'x' })).toBeNull()
    expect(inverseOf(card, { delegateActorId: 'someone' })).toBeNull()
  })

  it('undoes only while the task still holds what the change set', () => {
    const after: WorkItem = { ...task, status: { id: 'done', name: 'Done', semanticCategory: 'completed' }, labels: [urgent, { id: 'label-new', name: 'New', color: 'blue' }] }
    expect(stillAsSet(after, { statusId: 'done', addLabelIds: ['label-new'] })).toBe(true)
    // Someone moved it on since.
    expect(stillAsSet({ ...after, status: { id: 'review', name: 'Review', semanticCategory: 'started' } }, { statusId: 'done' })).toBe(false)
    expect(stillAsSet({ ...after, labels: [urgent] }, { addLabelIds: ['label-new'] })).toBe(false)
  })
})
