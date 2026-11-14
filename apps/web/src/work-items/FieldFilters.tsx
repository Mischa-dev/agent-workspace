import { useState } from 'react'

import {
  CATEGORY_LABELS,
  STATUS_CATEGORIES,
  behavesAs,
  clauseText,
  opLabel,
  opTakesValues,
  opsFor,
  type FieldClause,
  type FieldDefinition,
  type FieldOp,
} from '../fields/fieldModel'
import { Banner, Button, Checkbox, Icon, SelectField, TextField } from '../ui'
import { Popover } from '../ui/pickers'
import { useAssignableActors } from './hooks'
import { fieldClauses, withClauses } from './fieldFilterModel'
import type { ListState } from './listModel'
import type { UpdateListState } from './useListState'

/**
 * Chips for the list's custom field filters (AWD-89), beside the built-in
 * ones: each reads as a phrase ("Risk is High"), opens its editor and can
 * be removed. Saved views keep them like any other filter.
 */
export function FieldFilterChips({
  fields,
  onEdit,
  state,
  update,
}: {
  fields: readonly FieldDefinition[]
  onEdit: (index: number, anchor: HTMLElement) => void
  state: ListState
  update: UpdateListState
}) {
  const clauses = fieldClauses(state)
  return (
    <>
      {clauses.map((clause, index) => {
        const text = clauseText(clause, fields.find((field) => field.id === clause.field))
        return (
          <li className="aw-chipfilter" key={`${clause.field}-${index}`}>
            <button className="aw-chipfilter__text" onClick={(event) => onEdit(index, event.currentTarget)} title={text} type="button">
              {text}
            </button>
            <button
              aria-label={`Remove filter: ${text}`}
              className="aw-chipfilter__remove"
              onClick={() => update((current) => withClauses(current, fieldClauses(current).filter((_, at) => at !== index)))}
              type="button"
            >
              <Icon name="x" size={12} />
            </button>
          </li>
        )
      })}
    </>
  )
}

/** Picks how a field matches and what it matches, then applies the filter. */
export function FieldFilterEditor({
  anchor,
  clause,
  field,
  fits,
  onApply,
  onClose,
  projectId,
}: {
  anchor: HTMLElement
  clause: FieldClause | null
  field: FieldDefinition
  /** Whether the list's field filters, with this one applied, fit its URL and a saved view. */
  fits: (clause: FieldClause) => boolean
  onApply: (clause: FieldClause) => void
  onClose: () => void
  projectId: string
}) {
  const [tooMany, setTooMany] = useState(false)
  // Linked tasks and pages filter by whether they're set; the rest by value too.
  const ops = field.type === 'link' ? (['notEmpty', 'empty'] as FieldOp[]) : opsFor(field.type)
  const [op, setOp] = useState<FieldOp>(clause?.op ?? ops[0])
  const [values, setValues] = useState<string[]>(clause?.values ?? [])
  const people = useAssignableActors(projectId, '')
  const toggle = (value: string) => setValues(values.includes(value) ? values.filter((entry) => entry !== value) : [...values, value])
  // A record's maker filters as a person, its times as dates, its ID as a number.
  const like = behavesAs(field.type)
  const byCategory = op === 'categoryIs' || op === 'categoryIsNot'
  const choices = like === 'single_choice' || like === 'multiple_choice' || like === 'status'
  const needsValues = opTakesValues(op) && like !== 'checkbox'
  const ready = !needsValues || values.some((value) => value.trim())
  // A typed value box (not a list of options or people) takes one value for
  // these, and several separated by commas for a text field's "is".
  const typed = !choices && like !== 'person' && like !== 'checkbox'
  const oneValue = (next: FieldOp) => next === 'contains' || next === 'gt' || next === 'gte' || next === 'lt' || next === 'lte' || field.type !== 'text'
  const single = oneValue(op)
  const changeOp = (next: FieldOp) => {
    setOp(next)
    // Categories and options are different values.
    if ((next === 'categoryIs' || next === 'categoryIsNot') !== byCategory) setValues([])
    // Several values typed for "is" become one, written as the box showed
    // them, rather than staying several that a one-value match can't take.
    if (typed && oneValue(next) && values.length > 1) setValues([values.join(',')])
  }
  const apply = () => {
    if (!ready) return
    const stored = field.type === 'checkbox' && op === 'is' ? ['true'] : needsValues ? values.map((value) => value.trim()).filter(Boolean) : []
    const next: FieldClause = opTakesValues(op) ? { field: field.id, op, values: stored } : { field: field.id, op }
    // Said here, before the list changes, rather than dropping every filter later.
    if (!fits(next)) return setTooMany(true)
    onApply(next)
    onClose()
  }
  let chooser = null
  if (needsValues) {
    if (byCategory) {
      chooser = (
        <div role="group" aria-label={`${field.name} categories`}>
          {STATUS_CATEGORIES.map((category) => (
            <Checkbox checked={values.includes(category)} key={category} label={CATEGORY_LABELS[category]} onChange={() => toggle(category)} />
          ))}
        </div>
      )
    } else if (choices) {
      chooser = (
        <div role="group" aria-label={`${field.name} options`}>
          {field.options
            .filter((option) => !option.archived || values.includes(option.id))
            .map((option) => (
              <Checkbox checked={values.includes(option.id)} key={option.id} label={option.name} onChange={() => toggle(option.id)} />
            ))}
        </div>
      )
    } else if (like === 'person') {
      chooser = (
        <div role="group" aria-label="People">
          <Checkbox checked={values.includes('me')} label="Me" onChange={() => toggle('me')} />
          {(people.data ?? []).map((person) => (
            <Checkbox checked={values.includes(person.id)} key={person.id} label={person.displayName} onChange={() => toggle(person.id)} />
          ))}
        </div>
      )
    } else {
      chooser = (
        <TextField
          aria-label={`${field.name} value`}
          autoFocus
          compact
          inputMode={like === 'number' ? 'decimal' : undefined}
          onChange={(event) => setValues(single ? [event.target.value] : event.target.value.split(','))}
          type={like === 'date' ? 'date' : 'text'}
          value={values.join(single ? '' : ',')}
        />
      )
    }
  }
  return (
    <Popover anchor={anchor} label={`Filter by ${field.name}`} onClose={onClose} open width={300}>
      <form
        className="aw-filterbar__popover"
        onSubmit={(event) => {
          event.preventDefault()
          apply()
        }}
      >
        <SelectField aria-label={`How ${field.name} matches`} compact onChange={(event) => changeOp(event.target.value as FieldOp)} value={op}>
          {ops.map((entry) => (
            <option key={entry} value={entry}>
              {field.name} {opLabel(entry, field.type)}
            </option>
          ))}
        </SelectField>
        {chooser}
        {tooMany ? (
          <Banner tone="danger">Too many field filters or values for this list to keep. Choose fewer, or remove another field filter.</Banner>
        ) : null}
        <Button disabled={!ready} size="sm" type="submit">
          Apply
        </Button>
      </form>
    </Popover>
  )
}
