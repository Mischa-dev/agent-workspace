import { useState, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { listPages, memoryKeys } from '../memory/api'
import { Avatar, Button, TextArea, TextField, actorKind } from '../ui'
import { Calendar, ComboboxList, Popover } from '../ui/pickers'
import { useAssignableActors } from '../work-items/hooks'
import { useDebouncedValue } from '../work-items/pickerModel'
import { TaskSearchPicker } from '../work-items/TaskSearchPicker'
import { displayText, isEmpty, type FieldDefinition, type FieldReference, type FieldValue } from './fieldModel'

/**
 * The popover that changes one field's value, shaped by its type: a task's
 * fields, the bulk bar's change to many tasks at once, and a collection's
 * cells and record panel (AWD-90).
 */
export function FieldEditor({
  anchor,
  field,
  onClose,
  onLabel,
  onSet,
  projectId,
  references,
  taskId,
  value,
}: {
  anchor: HTMLElement
  field: FieldDefinition
  onClose: () => void
  /** A picked person, task or page as its picker named it (move dialog, AWD-102). */
  onLabel?: (label: string | null) => void
  onSet: (value: FieldValue | null) => void
  projectId: string
  references: readonly FieldReference[]
  /** The task being edited, which a link field can't point at. */
  taskId?: string
  value: FieldValue | undefined
}) {
  const pick = (next: FieldValue | null) => {
    onSet(next)
    onClose()
  }
  const named = (next: FieldValue | null, label: string | null) => {
    onLabel?.(next === null ? null : label)
    pick(next)
  }
  if (field.type === 'link' && field.target !== 'page' && field.target !== 'record') {
    return (
      <TaskSearchPicker
        allowTypedKey={false}
        anchor={anchor}
        exclude={taskId ? [taskId] : []}
        label={field.name}
        noneLabel={isEmpty(value) ? undefined : `Clear ${field.name}`}
        onClose={onClose}
        onPick={(target) => named(target.id, target.key || null)}
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
      body = <PersonEditor field={field} onClose={onClose} onSet={named} value={value} projectId={projectId} />
      break
    case 'link':
      body =
        field.target === 'record' ? (
          <RecordEditor field={field} onClose={onClose} onSet={named} projectId={projectId} value={value} />
        ) : (
          <PageEditor field={field} onClose={onClose} onSet={named} projectId={projectId} value={value} />
        )
      break
    // The task page edits a date with its own DatePicker; here (the bulk bar) it's the same calendar.
    case 'date':
      body = <Calendar label={field.name} onSelect={pick} value={typeof value === 'string' ? value : null} />
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
  onSet: (value: FieldValue | null, label: string | null) => void
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
      onSelect={(id) => {
        const person = (people.data ?? []).find((entry) => entry.id === id)
        onSet(id === '__none' ? null : id, person ? `${person.displayName} (@${person.handle})` : null)
      }}
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
  onSet: (value: FieldValue | null, label: string | null) => void
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
      onSelect={(id) => onSet(id === '__none' ? null : id, (pages.data?.pages ?? []).find((page) => page.id === id)?.title ?? null)}
      options={[
        ...(isEmpty(value) ? [] : [{ value: '__none', label: `Clear ${field.name}` }]),
        ...(pages.data?.pages ?? []).map((page) => ({ value: page.id, label: page.title })),
      ]}
      placeholder="Find a page"
      selected={typeof value === 'string' ? [value] : []}
    />
  )
}

type RecordHit = { id: string; title: string; number: number }

/** A record of the collection a record link holds (AWD-110), found by its title. */
function RecordEditor({
  field,
  onClose,
  onSet,
  projectId,
  value,
}: {
  field: FieldDefinition
  onClose: () => void
  onSet: (value: FieldValue | null, label: string | null) => void
  projectId: string
  value: FieldValue | undefined
}) {
  const [query, setQuery] = useState('')
  const debounced = useDebouncedValue(query, 200)
  const collection = field.collection ?? ''
  const records = useQuery({
    queryKey: ['project', projectId, 'collections', collection, 'records', 'pick', debounced.trim()],
    enabled: Boolean(collection),
    queryFn: async () => {
      const params = new URLSearchParams({ limit: '20' })
      if (debounced.trim()) params.set('search', debounced.trim())
      const payload = await requestJson<{ records?: Array<{ id?: unknown; title?: unknown; number?: unknown }> }>(
        `/api/v1/projects/${encodeURIComponent(projectId)}/collections/${encodeURIComponent(collection)}/records?${params.toString()}`,
      )
      return (payload.records ?? []).flatMap((record): RecordHit[] =>
        typeof record.id === 'string'
          ? [{ id: record.id, title: typeof record.title === 'string' ? record.title : '', number: typeof record.number === 'number' ? record.number : 0 }]
          : [],
      )
    },
  })
  const label = (record: RecordHit) => record.title || `Untitled #${record.number}`
  return (
    <ComboboxList
      emptyText={records.isError ? 'Records didn’t load. Try again.' : 'No records match'}
      label={field.name}
      loading={records.isFetching}
      onClose={onClose}
      onQueryChange={setQuery}
      onSelect={(id) => {
        const record = (records.data ?? []).find((entry) => entry.id === id)
        onSet(id === '__none' ? null : id, record ? label(record) : null)
      }}
      options={[
        ...(isEmpty(value) || query.trim() ? [] : [{ value: '__none', label: `Clear ${field.name}` }]),
        ...(records.data ?? []).map((record) => ({ value: record.id, label: label(record), description: `#${record.number}` })),
      ]}
      placeholder="Find a record"
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
