import { useState, type ReactNode } from 'react'

import { useFields } from '../fields/api'
import { FieldEditor } from '../fields/FieldEditor'
import { FieldValueView } from '../fields/FieldValueView'
import { displayText, isEmpty, type FieldDefinition, type FieldValue } from '../fields/fieldModel'
import { Icon } from '../ui'
import { DatePicker } from '../ui/pickers'
import type { WorkItem } from './types'
import type { TaskSave } from './useTaskSave'

/**
 * A task's custom fields (AWD-89), each edited in place and saved on its
 * own, as the built-in fields are: rows in the page's side rail, pills under
 * the title in the panel and on phones. Owners and admins add fields in the
 * project's settings.
 */
export function TaskCustomFields({
  control,
  disabled,
  item,
  render,
  save,
  tucked,
}: {
  /** The class each value's button takes: rail rows or pills. */
  control: string
  disabled: boolean
  item: WorkItem
  /** Wraps one field in its row or pill. */
  render: (label: string, value: ReactNode) => ReactNode
  save: TaskSave
  /** Whether an empty field waits behind More (pills only). */
  tucked: boolean
}) {
  const fields = useFields({ projectId: item.projectId })
  const [editing, setEditing] = useState<{ field: FieldDefinition; anchor: HTMLElement } | null>(null)
  const definitions = fields.data?.fields ?? []
  if (!definitions.length) return null
  const set = (field: FieldDefinition, value: FieldValue | null) => {
    // Against what the person sees now, their own change on its way
    // included, so putting a field back before that change lands still saves.
    if (save.baseFor(`field:${field.id}`) === JSON.stringify(value)) return
    save.save(`field:${field.id}`, { fieldValues: { [field.id]: value } })
  }
  return (
    <>
      {definitions.map((field) => {
        const value = item.fieldValues[field.id]
        if (tucked && isEmpty(value)) return null
        const shown = displayText(field, value, item.fieldReferences)
        if (field.type === 'date') {
          return render(
            field.name,
            <DatePicker
              className={`${control} aw-prop__date`}
              disabled={disabled}
              key={field.id}
              label={field.name}
              onChange={(next) => set(field, next)}
              placeholder={control === 'aw-pill' ? field.name : 'Empty'}
              value={typeof value === 'string' ? value : null}
            />,
          )
        }
        const content = isEmpty(value) ? (
          <span className="aw-prop__empty">{control === 'aw-pill' ? field.name : 'Empty'}</span>
        ) : (
          <FieldValueView field={field} references={item.fieldReferences} value={value} />
        )
        if (disabled) {
          return render(
            field.name,
            <span className={`${control} is-static${isEmpty(value) ? ' is-empty' : ''}`} key={field.id}>
              {content}
            </span>,
          )
        }
        if (field.type === 'checkbox') {
          return render(
            field.name,
            <button
              aria-label={`${field.name}: ${value === true ? 'checked' : 'not checked'}. Change`}
              aria-pressed={value === true}
              className={`${control}${value === true ? '' : ' is-empty'}`}
              key={field.id}
              onClick={() => set(field, value === true ? null : true)}
              type="button"
            >
              <Icon name={value === true ? 'circle-check' : 'unassigned'} size={16} />
              {control === 'aw-pill' || value !== true ? field.name : 'Yes'}
            </button>,
          )
        }
        return render(
          field.name,
          <button
            aria-haspopup="dialog"
            aria-label={`${field.name}: ${shown || 'empty'}. Change`}
            className={`${control}${isEmpty(value) ? ' is-empty' : ''}`}
            key={field.id}
            onClick={(event) => setEditing({ field, anchor: event.currentTarget })}
            type="button"
          >
            {content}
          </button>,
        )
      })}
      {editing ? (
        <FieldEditor
          anchor={editing.anchor}
          field={editing.field}
          onClose={() => setEditing(null)}
          onSet={(value) => set(editing.field, value)}
          projectId={item.projectId}
          references={item.fieldReferences}
          taskId={item.id}
          value={item.fieldValues[editing.field.id]}
        />
      ) : null}
    </>
  )
}
