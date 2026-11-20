import { fieldScope } from '../collections/api'
import { SelectField } from '../ui'
import { useFields } from './api'
import { ROLLUP_AGGREGATES, behavesAs, type FieldDefinition, type RollupAggregate } from './fieldModel'
import type { RollupDraft } from './rollupDraft'

/** Whether a field of the related records suits an aggregate. */
function suits(field: FieldDefinition, needs: (typeof ROLLUP_AGGREGATES)[number]['needs']) {
  if (needs === null || needs === 'any') return field.type !== 'button'
  return behavesAs(field) === needs
}

/**
 * A rollup's settings (T1.18): which relation it reads, how it puts the
 * related records' values together, and which of their fields. The related
 * collection's fields load once a relation is picked.
 */
export function RollupEditor({
  draft,
  fields,
  onChange,
  projectId,
}: {
  draft: RollupDraft
  /** The collection's own fields, for its relations. */
  fields: readonly FieldDefinition[]
  onChange: (draft: RollupDraft) => void
  projectId: string
}) {
  const relations = fields.filter((field) => field.type === 'relation' && !field.archived)
  const relation = relations.find((field) => field.id === draft.relation)
  // No relation yet: nothing to load (an empty project turns the query off).
  const related = useFields(relation?.collection ? fieldScope(projectId, relation.collection) : { projectId: '' }, false)
  const needs = ROLLUP_AGGREGATES.find((entry) => entry.aggregate === draft.aggregate)?.needs ?? null
  const choices = (related.data?.fields ?? []).filter((field) => suits(field, needs))
  if (!relations.length) {
    return <p className="aw-pagesection__desc">Add a Related records field first: a rollup reads the records it names.</p>
  }
  return (
    <>
      <SelectField label="Reads the records in" onChange={(event) => onChange({ ...draft, relation: event.target.value, field: '' })} value={draft.relation}>
        {relations.map((field) => (
          <option key={field.id} value={field.id}>
            {field.name}
          </option>
        ))}
      </SelectField>
      <SelectField
        label="Shows"
        onChange={(event) => {
          const aggregate = event.target.value as RollupAggregate
          onChange({ ...draft, aggregate, field: aggregate === 'count' ? '' : draft.field })
        }}
        value={draft.aggregate}
      >
        {ROLLUP_AGGREGATES.map((entry) => (
          <option key={entry.aggregate} value={entry.aggregate}>
            {entry.label}
          </option>
        ))}
      </SelectField>
      {needs !== null ? (
        <SelectField
          hint={related.isError ? 'Their fields didn’t load. Close this and try again.' : undefined}
          label="Of their field"
          onChange={(event) => onChange({ ...draft, field: event.target.value })}
          value={draft.field}
        >
          <option value="">{related.isPending ? 'Loading…' : choices.length ? 'Pick a field' : 'No field of theirs fits'}</option>
          {choices.map((field) => (
            <option key={field.id} value={field.id}>
              {field.name}
            </option>
          ))}
        </SelectField>
      ) : null}
    </>
  )
}
