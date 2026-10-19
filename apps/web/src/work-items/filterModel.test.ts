import { describe, expect, it } from 'vitest'

import { filterChips, withOperator, withValues } from './filterModel'
import type { ProjectLabel, WorkItemWorkflow } from './types'

const workflow = {
  statuses: [
    { id: 's1', name: 'Ready', semanticCategory: 'unstarted', position: 1 },
    { id: 's2', name: 'In Progress', semanticCategory: 'started', position: 2 },
    { id: 's3', name: 'Review', semanticCategory: 'review', position: 3 },
  ],
  types: [{ id: 't1', name: 'Bug' }],
} as unknown as WorkItemWorkflow
const labels = [
  { id: 'l2', name: 'Backend', color: '#000000' },
  { id: 'l1', name: 'Frontend', color: '#ffffff' },
] as unknown as ProjectLabel[]

describe('filter chips', () => {
  it('read as a sentence: the field, how it matches, then its values in workflow order', () => {
    expect(filterChips({ statusId: 's2,s1' }, { workflow })).toEqual([
      expect.objectContaining({ label: 'Status', verb: 'is any of', values: 'Ready, In Progress', text: 'Status is any of Ready, In Progress' }),
    ])
    expect(filterChips({ notStatusId: 's1,s2,s3' }, { workflow })[0]).toMatchObject({ verb: 'is none of', values: '3 statuses', text: 'Status is none of Ready, In Progress, Review' })
    expect(filterChips({ priority: 'urgent' }, {})[0]).toMatchObject({ text: 'Priority is Urgent' })
    expect(filterChips({ notTypeId: 't1' }, { workflow })[0]).toMatchObject({ text: 'Type is not Bug' })
    expect(filterChips({ labelIds: 'l1,l2' }, { labels })[0]).toMatchObject({ text: 'Labels include all of Backend, Frontend' })
    expect(filterChips({ anyLabelIds: 'none,l1' }, { labels })[0]).toMatchObject({ text: 'Labels include any of No labels, Frontend' })
    expect(filterChips({ notLabelIds: 'l2' }, { labels })[0]).toMatchObject({ text: 'Labels don’t include Backend' })
  })

  it('say a lone "none" in a few words, and name what is gone', () => {
    expect(filterChips({ assigneeActorId: 'none' }, {})[0]).toMatchObject({ verb: '', values: 'Unassigned' })
    expect(filterChips({ notAssigneeActorId: 'none' }, {})[0]).toMatchObject({ values: 'Assigned' })
    expect(filterChips({ notLabelIds: 'none' }, {})[0]).toMatchObject({ values: 'Has labels' })
    expect(filterChips({ milestoneId: 'none' }, {})[0]).toMatchObject({ values: 'No milestone' })
    expect(filterChips({ assigneeActorId: 'a1,a2' }, { actorNames: new Map([['a1', 'Ana']]) })[0]).toMatchObject({ text: 'Assignee is any of Ana, someone' })
    expect(filterChips({ statusId: 'gone' }, { workflow })[0]).toMatchObject({ text: 'Status is a removed status' })
  })

  it('list one chip per way a field matches, beside the other filters', () => {
    const chips = filterChips({ statusId: 's1', notStatusId: 's3', blocked: 'true', parentId: 'none' }, { workflow })
    expect(chips.map((chip) => chip.text)).toEqual(['Status is Ready', 'Status is not Review', 'Top-level only', 'Blocked'])
  })
})

describe('changing a filter', () => {
  it('moves values between "is" and "is not", joining what is there', () => {
    expect(withOperator({ statusId: 's1', notStatusId: 's2' }, 'status', 'any', 'not')).toEqual({ notStatusId: 's1,s2' })
    expect(withOperator({ priority: 'high' }, 'priority', 'any', 'any')).toEqual({ priority: 'high' })
  })

  it('leaves "No labels" out of all of several labels, unless it is all there is', () => {
    expect(withOperator({ anyLabelIds: 'l1,none' }, 'labels', 'any', 'all')).toEqual({ labelIds: 'l1' })
    expect(withOperator({ anyLabelIds: 'none' }, 'labels', 'any', 'all')).toEqual({ labelIds: 'none' })
  })

  it('keeps values sorted and once, and drops a filter left with none', () => {
    expect(withValues({}, 'statusId', ['s2', 'none', 's1', 's2'])).toEqual({ statusId: 's1,s2,none' })
    expect(withValues({ statusId: 's1' }, 'statusId', [])).toEqual({})
  })
})
