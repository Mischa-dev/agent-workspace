import { describe, expect, it } from 'vitest'

import { PRIORITY_FIELD, PRIORITY_OPTIONS, describeFilter, ruleFilter, snoozeChoices } from './triageModel'

const label = (value: string) => value.charAt(0).toUpperCase() + value.slice(1)
const types = (id: string) => (id === 't1' ? 'Bug' : undefined)

describe('triage rules', () => {
  it('build a filter from the form and read it back', () => {
    expect(ruleFilter('', '')).toEqual([])
    const filter = ruleFilter('urgent', 't1')
    expect(filter[0]).toEqual({ field: PRIORITY_FIELD, op: 'is', values: [PRIORITY_OPTIONS.urgent] })
    expect(describeFilter(filter, label, types)).toBe('Priority is Urgent and type is Bug')
    expect(describeFilter([], label, types)).toBe('Every task')
  })

  it('call a filter the form did not write "some tasks"', () => {
    expect(describeFilter([{ field: 'other', op: 'is', values: ['x'] }], label, types)).toBe('Some tasks')
    expect(describeFilter(ruleFilter('', 'gone'), label, types)).toBe('Some tasks')
  })
})

describe('snooze choices', () => {
  it('end at nine in the morning, next week on a Monday', () => {
    const sunday = new Date(2026, 10, 15, 22, 30)
    const [tomorrow, week, month] = snoozeChoices(sunday)
    expect(tomorrow.until).toEqual(new Date(2026, 10, 16, 9))
    expect(week.until).toEqual(new Date(2026, 10, 16, 9))
    expect(week.until.getDay()).toBe(1)
    expect(month.until).toEqual(new Date(2026, 11, 15, 9))
    const monday = new Date(2026, 10, 16, 8)
    expect(snoozeChoices(monday)[1].until).toEqual(new Date(2026, 10, 23, 9))
  })
})
