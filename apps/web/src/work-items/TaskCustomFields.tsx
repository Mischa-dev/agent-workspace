import { useState, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'

import { useFields } from '../fields/api'
import { FieldValueView } from '../fields/FieldValueView'
import { displayText, isEmpty, type FieldDefinition, type FieldReference, type FieldValue } from '../fields/fieldModel'
import { listPages, memoryKeys } from '../memory/api'
import { Avatar, Button, Icon, TextArea, TextField, actorKind } from '../ui'
import { ComboboxList, DatePicker, Popover } from '../ui/pickers'
import { useAssignableActors } from './hooks'
import { useDebouncedValue } from './pickerModel'
import { TaskSearchPicker } from './TaskSearchPicker'
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

/** The popover that changes one field's value, shaped by its type. */
function FieldEditor({
  anchor,
  field,
  onClose,
  onSet,
  projectId,
  references,
  taskId,
  value,
}: {
  anchor: HTMLElement
  field: FieldDefinition
  onClose: () => void
  onSet: (value: FieldValue | null) => void
  projectId: string
  references: readonly FieldReference[]
  taskId: string
  value: FieldValue | undefined
}) {
  const pick = (next: FieldValue | null) => {
    onSet(next)
    onClose()
  }
  if (field.type === 'link' && field.target !== 'page') {
    return (
      <TaskSearchPicker
        allowTypedKey={false}
        anchor={anchor}
        exclude={[taskId]}
        label={field.name}
        noneLabel={isEmpty(value) ? undefined : `Clear ${field.name}`}
        onClose={onClose}
        onPick={(target) => pick(target.id)}
        projectId={projectId}
      />
    )
  }
  let body: ReactNode
  switch (field.type) {
    case 'single_choice':
    case 'multiple_choice':
      body = <ChoiceEditor field={field} onClose={onClose} onSet={onSet} value={value} />
      break
    case 'person':
      body = <PersonEditor field={field} onClose={onClose} onSet={pick} value={value} projectId={projectId} />
      break
    case 'link':
      body = <PageEditor field={field} onClose={onClose} onSet={pick} projectId={projectId} value={value} />
      break
    default:
      body = <TypedEditor field={field} onSet={pick} references={references} value={value} />
  }
  return (
    <Popover anchor={anchor} label={field.name} onClose={onClose} open width={field.type === 'text' ? 360 : 300}>
      {body}
    </Popover>
  )
}

function ChoiceEditor({
  field,
  onClose,
  onSet,
  value,
}: {
  field: FieldDefinition
  onClose: () => void
  onSet: (value: FieldValue | null) => void
  value: FieldValue | undefined
}) {
  const multiple = field.type === 'multiple_choice'
  const [chosen, setChosen] = useState<string[]>(Array.isArray(value) ? value : typeof value === 'string' ? [value] : [])
  const options = field.options
    .filter((option) => !option.archived || chosen.includes(option.id))
    .map((option) => ({
      value: option.id,
      label: option.name,
      leading: <span aria-hidden className="aw-label__dot" style={{ backgroundColor: option.color }} />,
    }))
  return (
    <ComboboxList
      emptyText={field.options.length ? 'No options match' : 'This field has no options yet. A project admin can add some in Settings.'}
      footer={
        chosen.length ? (
          <Button
            onClick={() => {
              setChosen([])
              onSet(null)
              onClose()
            }}
            size="sm"
            variant="ghost"
          >
            Clear {field.name}
          </Button>
        ) : undefined
      }
      label={field.name}
      multiple={multiple}
      onClose={multiple ? undefined : onClose}
      onSelect={(id) => {
        if (!multiple) return onSet(id)
        const next = chosen.includes(id) ? chosen.filter((entry) => entry !== id) : [...chosen, id]
        // Kept in the field's order, as the server stores them.
        const ordered = field.options.map((option) => option.id).filter((entry) => next.includes(entry))
        setChosen(ordered)
        onSet(ordered.length ? ordered : null)
      }}
      options={options}
      placeholder={`Find ${multiple ? 'options' : 'an option'}`}
      selected={chosen}
    />
  )
}

function PersonEditor({
  field,
  onClose,
  onSet,
  projectId,
  value,
}: {
  field: FieldDefinition
  onClose: () => void
  onSet: (value: FieldValue | null) => void
  projectId: string
  value: FieldValue | undefined
}) {
  const [query, setQuery] = useState('')
  const debounced = useDebouncedValue(query, 200)
  const people = useAssignableActors(projectId, debounced)
  return (
    <ComboboxList
      emptyText={people.isError ? 'People didn’t load. Try again.' : 'No one matches'}
      label={field.name}
      loading={people.isFetching}
      onClose={onClose}
      onQueryChange={setQuery}
      onSelect={(id) => onSet(id === '__none' ? null : id)}
      options={[
        ...(isEmpty(value) || query.trim() ? [] : [{ value: '__none', label: `Clear ${field.name}` }]),
        ...(people.data ?? []).map((person) => ({
          value: person.id,
          label: person.displayName,
          description: `@${person.handle}`,
          keywords: person.handle,
          leading: <Avatar kind={actorKind(person.kind)} name={person.displayName} seed={person.id} size={18} />,
        })),
      ]}
      placeholder="Find someone"
      selected={typeof value === 'string' ? [value] : []}
    />
  )
}

function PageEditor({
  field,
  onClose,
  onSet,
  projectId,
  value,
}: {
  field: FieldDefinition
  onClose: () => void
  onSet: (value: FieldValue | null) => void
  projectId: string
  value: FieldValue | undefined
}) {
  const pages = useQuery({ queryKey: memoryKeys.pages(projectId), queryFn: () => listPages(projectId) })
  return (
    <ComboboxList
      emptyText={pages.isError ? 'Pages didn’t load. Try again.' : 'No pages match'}
      label={field.name}
      loading={pages.isFetching}
      onClose={onClose}
      onSelect={(id) => onSet(id === '__none' ? null : id)}
      options={[
        ...(isEmpty(value) ? [] : [{ value: '__none', label: `Clear ${field.name}` }]),
        ...(pages.data?.pages ?? []).map((page) => ({ value: page.id, label: page.title })),
      ]}
      placeholder="Find a page"
      selected={typeof value === 'string' ? [value] : []}
    />
  )
}

/** Text, a number or a web address: typed, then set with Enter or Set. */
function TypedEditor({
  field,
  onSet,
  references,
  value,
}: {
  field: FieldDefinition
  onSet: (value: FieldValue | null) => void
  references: readonly FieldReference[]
  value: FieldValue | undefined
}) {
  const [typed, setTyped] = useState(isEmpty(value) ? '' : String(value))
  const [problem, setProblem] = useState<string | null>(null)
  const submit = (event: { preventDefault: () => void }) => {
    event.preventDefault()
    const trimmed = typed.trim()
    if (!trimmed) return onSet(null)
    if (field.type === 'number') {
      const parsed = Number(trimmed.replace(/[,_]/g, ''))
      if (!Number.isFinite(parsed) || Math.abs(parsed) > 1e15) return setProblem('Use a number, such as 3 or 2.5.')
      return onSet(parsed)
    }
    if (field.type === 'url' && /\s/.test(trimmed)) return setProblem('Use a web address, such as https://example.com.')
    onSet(trimmed)
  }
  const input =
    field.type === 'text' ? (
      <TextArea
        aria-label={field.name}
        autoFocus
        error={problem ?? undefined}
        maxLength={5000}
        onChange={(event) => setTyped(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) submit(event)
        }}
        rows={3}
        value={typed}
      />
    ) : (
      <TextField
        aria-label={field.name}
        autoFocus
        compact
        error={problem ?? undefined}
        inputMode={field.type === 'number' ? 'decimal' : field.type === 'url' ? 'url' : undefined}
        onChange={(event) => {
          setTyped(event.target.value)
          setProblem(null)
        }}
        placeholder={field.type === 'url' ? 'https://' : field.type === 'number' ? '0' : ''}
        value={typed}
      />
    )
  return (
    <form className="aw-estimate__form" onSubmit={submit}>
      {field.description ? <p className="aw-prop__empty">{field.description}</p> : null}
      {input}
      <div className="aw-estimate__row">
        <Button size="sm" type="submit">
          Set
        </Button>
        {!isEmpty(value) ? (
          <Button onClick={() => onSet(null)} size="sm" variant="ghost">
            Clear {displayText(field, value, references) ? field.name : ''}
          </Button>
        ) : null}
      </div>
    </form>
  )
}
