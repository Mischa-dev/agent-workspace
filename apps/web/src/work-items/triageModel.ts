/** When a snooze can end, from `now`: tomorrow, next Monday and in a month, each at 9 in the morning, local time. */
export function snoozeChoices(now: Date): Array<{ label: string; until: Date }> {
  const at = (days: number) => {
    const day = new Date(now)
    day.setDate(day.getDate() + days)
    day.setHours(9, 0, 0, 0)
    return day
  }
  const toMonday = ((8 - now.getDay()) % 7) || 7
  return [
    { label: 'Tomorrow', until: at(1) },
    { label: 'Next week', until: at(toMonday) },
    { label: 'In a month', until: at(30) },
  ]
}

/** The built-in Priority and Type fields of tasks, and Priority's options (the server's `collections::tasks`). */
export const PRIORITY_FIELD = '0192f5c0-0000-7000-8000-000000000003'
export const TYPE_FIELD = '0192f5c0-0000-7000-8000-000000000007'
export const PRIORITY_OPTIONS: Record<string, string> = {
  urgent: '0192f5c0-0000-7000-8000-0000000000a1',
  high: '0192f5c0-0000-7000-8000-0000000000a2',
  medium: '0192f5c0-0000-7000-8000-0000000000a3',
  low: '0192f5c0-0000-7000-8000-0000000000a4',
}

export type RuleClause = { field: string; op: string; values?: string[] }

/** A rule's filter from the simple form: a priority and a type, each optional; none matches every task. */
export function ruleFilter(priority: string, typeId: string): RuleClause[] {
  const clauses: RuleClause[] = []
  const option = PRIORITY_OPTIONS[priority]
  if (option) clauses.push({ field: PRIORITY_FIELD, op: 'is', values: [option] })
  if (typeId) clauses.push({ field: TYPE_FIELD, op: 'is', values: [typeId] })
  return clauses
}

/** What a rule's filter asks, in words, for the parts the form writes; anything else is "some tasks". */
export function describeFilter(filter: unknown, priorityLabel: (value: string) => string, typeName: (id: string) => string | undefined): string {
  if (!Array.isArray(filter) || filter.length === 0) return 'Every task'
  const parts = filter.map((clause: RuleClause) => {
    const value = clause.values?.[0] ?? ''
    if (clause.field === PRIORITY_FIELD && clause.op === 'is') {
      const priority = Object.entries(PRIORITY_OPTIONS).find(([, id]) => id === value)?.[0]
      return priority ? `priority is ${priorityLabel(priority)}` : null
    }
    if (clause.field === TYPE_FIELD && clause.op === 'is') {
      const name = typeName(value)
      return name ? `type is ${name}` : null
    }
    return null
  })
  if (parts.some((part) => part === null)) return 'Some tasks'
  const text = parts.join(' and ')
  return text.charAt(0).toUpperCase() + text.slice(1)
}
