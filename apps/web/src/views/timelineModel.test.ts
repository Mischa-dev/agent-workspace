import { describe, expect, it } from 'vitest'

import type { TimelineMilestone, TimelineTask } from './api'
import { slips } from './timelineModel'

const task = (id: string, startDate: string | null, dueDate: string | null, milestoneId: string | null = 'm1'): TimelineTask => ({
  id,
  key: `T-${id}`,
  title: id,
  statusName: 'Todo',
  statusCategory: 'unstarted',
  priority: 'medium',
  startDate,
  dueDate,
  estimate: null,
  milestoneId,
  parentId: null,
  assigneeName: null,
  blocked: false,
  version: 1,
})

const milestone: TimelineMilestone = { id: 'm1', name: 'Beta', targetDate: '2026-11-10', overdue: false, progress: 0, version: 1 }

describe('what slips', () => {
  const tasks = [task('a', '2026-11-01', '2026-11-05'), task('b', '2026-11-06', '2026-11-09'), task('c', null, '2026-11-02', null)]

  it('pulling a milestone in flags the tasks that would end after it', () => {
    const result = slips(tasks, [milestone], [], { kind: 'milestone', id: 'm1', days: -3 })
    expect([...result.tasks.keys()]).toEqual(['b'])
    expect(result.milestones.has('m1')).toBe(true)
  })

  it('pushing a milestone out slips nothing', () => {
    expect(slips(tasks, [milestone], [], { kind: 'milestone', id: 'm1', days: 7 }).tasks.size).toBe(0)
  })

  it('moving a task later flags what it blocks and its milestone', () => {
    const result = slips(tasks, [milestone], [{ blockingId: 'a', blockedId: 'b' }], { kind: 'task', id: 'a', days: 6, edge: 'move' })
    expect(result.tasks.get('b')).toBe('Starts before T-a is done')
    expect(result.tasks.get('a')).toBe('Ends after Beta')
  })

  it('leaves out what was already late before the change', () => {
    const late = [task('d', '2026-11-12', '2026-11-20')]
    expect(slips(late, [milestone], [], { kind: 'milestone', id: 'm1', days: -1 }).tasks.size).toBe(0)
  })
})
