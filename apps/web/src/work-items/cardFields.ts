import { useMemo } from 'react'

import { useProjectExperience } from '../experience/layout'
import type { PickerField } from './FieldPicker'

/** Which of a task's marks rows and cards show, as the project's layout says. */
export type CardFields = { keys: boolean; priority: boolean; dates: boolean; labels: boolean; milestone: boolean }

export const ALL_FIELDS: CardFields = { keys: true, priority: true, dates: true, labels: true, milestone: true }

/** Whether the list and board offer a field's picker, from the bulk bar or a key: not when the layout hides the field. */
export function pickerShown(field: PickerField, fields: CardFields): boolean {
  if (field === 'labels') return fields.labels
  if (field === 'priority') return fields.priority
  if (field === 'milestone') return fields.milestone
  return true
}

export function useCardFields(projectId: string): CardFields {
  const experience = useProjectExperience(projectId)
  const keys = experience.shows('tech.task_keys')
  const priority = experience.shows('task.priority')
  const dates = experience.shows('task.dates')
  const labels = experience.shows('task.labels')
  const milestone = experience.shows('task.milestone')
  return useMemo(() => ({ keys, priority, dates, labels, milestone }), [keys, priority, dates, labels, milestone])
}
