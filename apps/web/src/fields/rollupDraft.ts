import type { FieldDefinition, RollupAggregate } from './fieldModel'

/** A rollup as the field dialog edits it: the relation, the related records' field (by ID, '' to count), and how. */
export type RollupDraft = { relation: string; field: string; aggregate: RollupAggregate }

export function rollupDraft(field: FieldDefinition | null, fields: readonly FieldDefinition[]): RollupDraft {
  const relation = field?.rollup?.relation ?? fields.find((entry) => entry.type === 'relation' && !entry.archived)?.id ?? ''
  return { relation, field: field?.rollup?.target ?? '', aggregate: field?.rollup?.aggregate ?? 'count' }
}
