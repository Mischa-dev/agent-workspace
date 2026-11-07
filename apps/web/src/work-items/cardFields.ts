import { useMemo } from 'react'

import { useProjectExperience } from '../experience/layout'
import { useFields } from '../fields/api'
import type { FieldDefinition } from '../fields/fieldModel'
import type { PickerField } from './FieldPicker'

/** Which of a task's marks rows and cards show, as the project's layout says, and the custom fields cards show (AWD-89). */
export type CardFields = {
  keys: boolean
  priority: boolean
  dates: boolean
  labels: boolean
  milestone: boolean
  custom: readonly FieldDefinition[]
}

export const ALL_FIELDS: CardFields = { keys: true, priority: true, dates: true, labels: true, milestone: true, custom: [] }

/** Whether the list and board offer a field's picker, from the bulk bar or a key: not when the layout hides the field. */
export function pickerShown(field: PickerField, fields: CardFields): boolean {
  if (field === 'labels') return fields.labels
  if (field === 'priority') return fields.priority
  if (field === 'milestone') return fields.milestone
  return true
}

const NO_FIELDS: readonly FieldDefinition[] = []

export function useCardFields(projectId: string): CardFields {
  const experience = useProjectExperience(projectId)
  const keys = experience.shows('tech.task_keys')
  const priority = experience.shows('task.priority')
  const dates = experience.shows('task.dates')
  const labels = experience.shows('task.labels')
  const milestone = experience.shows('task.milestone')
  const showCustom = experience.shows('task.custom_fields')
  const projectFields = useFields({ projectId })
  const custom = showCustom ? (projectFields.data?.fields ?? NO_FIELDS) : NO_FIELDS
  return useMemo(() => ({ keys, priority, dates, labels, milestone, custom }), [keys, priority, dates, labels, milestone, custom])
}
