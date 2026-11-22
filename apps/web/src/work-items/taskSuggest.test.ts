import { describe, expect, it } from 'vitest'

import { suggestedUpdate } from './taskSuggest'

describe('suggestedUpdate', () => {
  it('sends the panel’s ids, and clears with the clear flags', () => {
    expect(suggestedUpdate('t1', 4, { statusId: 's2', assigneeActorId: null, estimate: null, dueDate: null, addLabelIds: ['l1'] })).toEqual({
      taskId: 't1',
      expectedVersion: 4,
      status: 's2',
      clearAssignee: true,
      clearEstimate: true,
      dueDate: '',
      addLabels: ['l1'],
    })
    expect(suggestedUpdate('t1', 4, { fieldValues: { f1: 'o1', f2: null } })).toEqual({
      taskId: 't1',
      expectedVersion: 4,
      fields: { f1: 'o1', f2: null },
    })
  })

  it('refuses what only an editor can change', () => {
    expect(suggestedUpdate('t1', 4, { milestoneId: 'm1' })).toBeNull()
    expect(suggestedUpdate('t1', 4, { title: 'New', delegateActorId: 'a1' })).toBeNull()
    expect(suggestedUpdate('t1', 4, { blockedReason: null })).toBeNull()
    expect(suggestedUpdate('t1', 4, {})).toBeNull()
  })
})
