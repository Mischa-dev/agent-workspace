import { useState } from 'react'

import {
  Banner,
  Button,
  Dialog,
  IconButton,
  Row,
  RowList,
  Section,
  SelectField,
  SettingsSkeleton,
  TextField,
  describeError,
  describeLoadError,
  showToast,
} from '../ui'
import { useFieldMutations, useFields, type FieldScope, type OptionInput } from './api'
import { CATEGORY_LABELS, FIELD_TYPES, STATUS_CATEGORIES, isSystemType, typeLabel, type FieldDefinition, type FieldType, type StatusCategory } from './fieldModel'

/** Named colors the server takes, in the order labels offer them. */
const COLORS = ['red', 'orange', 'yellow', 'green', 'teal', 'blue', 'purple', 'pink', 'gray'] as const

/**
 * A scope's custom fields (a project's task fields now; a collection's
 * later): everyone sees them, owners and admins add, rename, reorder,
 * archive and restore them, and edit a choice field's options. Archiving
 * keeps every value, so restoring brings them back.
 */
export function FieldManager({ scope }: { scope: FieldScope }) {
  const [showArchived, setShowArchived] = useState(false)
  const fields = useFields(scope, true)
  const { update } = useFieldMutations(scope)
  const [editing, setEditing] = useState<FieldDefinition | 'new' | null>(null)
  if (fields.isPending) return <SettingsSkeleton label="Loading fields" />
  if (fields.isError) {
    return (
      <Banner onRetry={() => void fields.refetch()} title="The fields didn’t load" tone="danger">
        {describeLoadError(fields.error)}
      </Banner>
    )
  }
  const { canManage } = fields.data
  const active = fields.data.fields.filter((field) => !field.archived)
  const archived = fields.data.fields.filter((field) => field.archived)
  const change = (field: FieldDefinition, patch: { position?: number; archived?: boolean }, done: string) =>
    update.mutate(
      { fieldId: field.id, expectedVersion: field.version, idempotencyKey: crypto.randomUUID(), ...patch },
      {
        onSuccess: () => showToast({ title: done, tone: 'success' }),
        onError: (error) => showToast({ title: 'That didn’t save', description: describeError(error).message, tone: 'danger' }),
      },
    )
  const row = (field: FieldDefinition, index: number) => (
    <Row
      actions={
        canManage ? (
          field.archived ? (
            <Button disabled={update.isPending} onClick={() => change(field, { archived: false }, `${field.name} restored`)} size="sm" variant="ghost">
              Restore
            </Button>
          ) : (
            <>
              <IconButton
                disabled={index === 0 || update.isPending}
                icon="arrow-up"
                label={`Move ${field.name} up`}
                onClick={() => change(field, { position: index }, `${field.name} moved up`)}
                size="sm"
              />
              <IconButton
                disabled={index === active.length - 1 || update.isPending}
                icon="arrow-down"
                label={`Move ${field.name} down`}
                onClick={() => change(field, { position: index + 2 }, `${field.name} moved down`)}
                size="sm"
              />
              <Button onClick={() => setEditing(field)} size="sm" variant="ghost">
                Edit
              </Button>
              <Button disabled={update.isPending} onClick={() => change(field, { archived: true }, `${field.name} archived`)} size="sm" variant="ghost">
                Archive
              </Button>
            </>
          )
        ) : null
      }
      key={field.id}
      meta={[
        typeLabel(field.type),
        field.type === 'link' ? (field.target === 'page' ? 'to a page' : field.target === 'record' ? 'to a record' : 'to a task') : null,
        field.options.filter((option) => !option.archived).length
          ? field.options
              .filter((option) => !option.archived)
              .map((option) => option.name)
              .join(', ')
          : null,
        field.description || null,
      ]
        .filter(Boolean)
        .join(' · ')}
      title={field.name}
    />
  )
  return (
    <>
      {!canManage ? <Banner icon="lock">Only the project’s owners and admins can change its fields. Anyone in it can fill them in.</Banner> : null}
      <Section
        actions={
          canManage ? (
            <Button icon="plus" onClick={() => setEditing('new')} size="sm">
              Add field
            </Button>
          ) : null
        }
        description="Every task has these, in this order. Agents read and set them by name."
        id="fields-heading"
        title="Fields"
      >
        {active.length ? (
          <RowList label="Fields">{active.map(row)}</RowList>
        ) : (
          <p className="aw-pagesection__desc">No fields yet. {canManage ? 'Add one to track something every task should say, such as Risk or Customer.' : ''}</p>
        )}
      </Section>
      {archived.length ? (
        <Section
          actions={
            <Button onClick={() => setShowArchived(!showArchived)} size="sm" variant="ghost">
              {showArchived ? 'Hide' : `Show ${archived.length}`}
            </Button>
          }
          description="Hidden everywhere. Tasks keep their values for when a field is restored."
          id="fields-archived-heading"
          title="Archived"
        >
          {showArchived ? <RowList label="Archived fields">{archived.map(row)}</RowList> : null}
        </Section>
      ) : null}
      {editing ? <FieldDialog field={editing === 'new' ? null : editing} onClose={() => setEditing(null)} scope={scope} /> : null}
    </>
  )
}

type OptionDraft = OptionInput & { key: string }

/**
 * Adds a field to a scope, or changes one: its name, description and a choice
 * field's options (a type never changes). Collections (AWD-90) open it from a
 * table's columns; `records` names what holds the values, for the copy.
 */
export function FieldDialog({
  field,
  onClose,
  onSaved,
  linkable = [],
  records = 'tasks',
  scope,
}: {
  /** Collections a new link field can hold the records of (AWD-110). */
  linkable?: ReadonlyArray<{ id: string; name: string }>
  field: FieldDefinition | null
  onClose: () => void
  /** The field as it was saved, before the dialog closes. */
  onSaved?: (saved: FieldDefinition) => void
  records?: string
  scope: FieldScope
}) {
  const { create, update } = useFieldMutations(scope)
  const [name, setName] = useState(field?.name ?? '')
  const [type, setType] = useState<FieldType>(field?.type ?? 'text')
  const [description, setDescription] = useState(field?.description ?? '')
  // "task", "page", or "record:<collection ID>".
  const [target, setTarget] = useState('task')
  const [options, setOptions] = useState<OptionDraft[]>(
    () =>
      field?.options
        .filter((option) => !option.archived)
        .map((option) => ({ key: option.id, id: option.id, name: option.name, color: option.color, category: option.category ?? undefined })) ?? [],
  )
  const [problem, setProblem] = useState<string | null>(null)
  const status = type === 'status'
  // A new status field without options gets the server's three.
  const choices = type === 'single_choice' || type === 'multiple_choice' || (status && (Boolean(field) || options.length > 0))
  // Tasks have their own status and record columns; a collection can have any type.
  const offered = FIELD_TYPES.filter((entry) => scope.path !== undefined || entry.tasks !== false)
  const pending = create.isPending || update.isPending
  const submit = () => {
    setProblem(null)
    if (!name.trim()) return setProblem('Give the field a name.')
    const optionInputs = choices
      ? options
          .filter((option) => option.name.trim())
          .map(({ id, name: optionName, color, category }) => ({ id, name: optionName.trim(), color, ...(status && category ? { category } : {}) }))
      : undefined
    const done = {
      onSuccess: (saved: FieldDefinition) => {
        onSaved?.(saved)
        onClose()
      },
      onError: (error: unknown) => setProblem(describeError(error).message),
    }
    if (field) {
      update.mutate(
        {
          fieldId: field.id,
          expectedVersion: field.version,
          idempotencyKey: crypto.randomUUID(),
          name: name.trim(),
          description: description.trim(),
          ...(optionInputs ? { options: optionInputs } : {}),
        },
        done,
      )
    } else {
      create.mutate(
        {
          idempotencyKey: crypto.randomUUID(),
          name: name.trim(),
          type,
          description: description.trim(),
          ...(optionInputs ? { options: optionInputs } : {}),
          ...(type === 'link' ? { target } : {}),
        },
        done,
      )
    }
  }
  return (
    <Dialog
      dirty
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={pending} type="submit">
            {field ? 'Save' : 'Add field'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        submit()
      }}
      open
      title={field ? `Edit ${field.name}` : 'Add a field'}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      <TextField autoFocus label="Name" maxLength={60} onChange={(event) => setName(event.target.value)} required value={name} />
      {field ? (
        <p className="aw-pagesection__desc">Type: {typeLabel(field.type)}. A field’s type can’t change; add a new field for another.</p>
      ) : (
        <SelectField label="Type" onChange={(event) => setType(event.target.value as FieldType)} value={type}>
          {offered.map((entry) => (
            <option key={entry.type} value={entry.type}>
              {entry.label}: {entry.hint}
            </option>
          ))}
        </SelectField>
      )}
      {!field && type === 'link' ? (
        <SelectField label="Links to" onChange={(event) => setTarget(event.target.value)} value={target}>
          <option value="task">A task</option>
          <option value="page">A page</option>
          {linkable.map((collection) => (
            <option key={collection.id} value={`record:${collection.id}`}>
              A record in {collection.name}
            </option>
          ))}
        </SelectField>
      ) : null}
      <TextField
        hint="Optional. Shown when someone fills it in."
        label="Description"
        maxLength={300}
        onChange={(event) => setDescription(event.target.value)}
        value={description}
      />
      {!field && isSystemType(type) ? <p className="aw-pagesection__desc">Filled in from each {records === 'tasks' ? 'task' : 'record'} itself; nobody edits it.</p> : null}
      {!field && status && !options.length ? (
        <p className="aw-pagesection__desc">
          Starts with Not started, In progress and Done.{' '}
          <Button
            onClick={() =>
              setOptions([
                { key: crypto.randomUUID(), name: 'Not started', category: 'unstarted' },
                { key: crypto.randomUUID(), name: 'In progress', category: 'started' },
                { key: crypto.randomUUID(), name: 'Done', category: 'completed' },
              ])
            }
            size="sm"
            variant="ghost"
          >
            Change the options
          </Button>
        </p>
      ) : null}
      {choices ? (
        <fieldset className="aw-fieldset">
          <legend className="aw-field__label">Options</legend>
          {options.map((option, index) => (
            <div className="aw-estimate__row" key={option.key}>
              <TextField
                aria-label={`Option ${index + 1}`}
                compact
                maxLength={60}
                onChange={(event) =>
                  setOptions(options.map((entry) => (entry.key === option.key ? { ...entry, name: event.target.value } : entry)))
                }
                value={option.name}
              />
              <SelectField
                aria-label={`Color of option ${index + 1}`}
                compact
                onChange={(event) =>
                  setOptions(options.map((entry) => (entry.key === option.key ? { ...entry, color: event.target.value || undefined } : entry)))
                }
                value={COLORS.includes(option.color as (typeof COLORS)[number]) ? option.color : ''}
              >
                <option value="">{option.color && !COLORS.includes(option.color as (typeof COLORS)[number]) ? 'Current color' : 'Automatic'}</option>
                {COLORS.map((color) => (
                  <option key={color} value={color}>
                    {color[0].toUpperCase() + color.slice(1)}
                  </option>
                ))}
              </SelectField>
              {status ? (
                <SelectField
                  aria-label={`Category of option ${index + 1}`}
                  compact
                  onChange={(event) =>
                    setOptions(options.map((entry) => (entry.key === option.key ? { ...entry, category: event.target.value as StatusCategory } : entry)))
                  }
                  value={option.category ?? 'unstarted'}
                >
                  {STATUS_CATEGORIES.map((category) => (
                    <option key={category} value={category}>
                      {CATEGORY_LABELS[category]}
                    </option>
                  ))}
                </SelectField>
              ) : null}
              <IconButton
                icon="x"
                label={`Remove option ${option.name || index + 1}`}
                onClick={() => setOptions(options.filter((entry) => entry.key !== option.key))}
                size="sm"
              />
            </div>
          ))}
          <Button
            icon="plus"
            onClick={() => setOptions([...options, { key: crypto.randomUUID(), name: '' }])}
            size="sm"
            variant="ghost"
          >
            Add option
          </Button>
          {field ? <p className="aw-pagesection__desc">A removed option is archived: {records} that have it keep it.</p> : null}
        </fieldset>
      ) : null}
    </Dialog>
  )
}
