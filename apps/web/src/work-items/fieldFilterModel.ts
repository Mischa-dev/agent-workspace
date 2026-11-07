import { MAX_FIELD_FILTERS_CHARACTERS, clausesParam, parseClauses, type FieldClause } from '../fields/fieldModel'
import { withFilter, type ListState } from './listModel'

/** The list's custom field filters (AWD-89), read from its URL. */
export function fieldClauses(state: ListState): FieldClause[] {
  return parseClauses(state.filters.fieldFilters)
}

/**
 * Whether these field filters fit the list's URL and a saved view. The
 * filter editor refuses a change that doesn't, rather than writing a URL
 * the list would then read as no filters at all.
 */
export function clausesFit(clauses: readonly FieldClause[]): boolean {
  return (clausesParam(clauses)?.length ?? 0) <= MAX_FIELD_FILTERS_CHARACTERS
}

/** The list's field filters with the one at `index` replaced, or `clause` added (index null). */
export function placeClause(clauses: readonly FieldClause[], index: number | null, clause: FieldClause): FieldClause[] {
  return index === null ? [...clauses, clause] : clauses.map((entry, at) => (at === index ? clause : entry))
}

/** The list with these field filters, written to its URL. */
export function withClauses(state: ListState, clauses: readonly FieldClause[]): ListState {
  return withFilter(state, 'fieldFilters', clausesParam(clauses))
}
