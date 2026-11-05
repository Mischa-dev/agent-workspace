import { clausesParam, parseClauses, type FieldClause } from '../fields/fieldModel'
import { withFilter, type ListState } from './listModel'

/** The list's custom field filters (AWD-89), read from its URL. */
export function fieldClauses(state: ListState): FieldClause[] {
  return parseClauses(state.filters.fieldFilters)
}

/** The list with these field filters, written to its URL. */
export function withClauses(state: ListState, clauses: readonly FieldClause[]): ListState {
  return withFilter(state, 'fieldFilters', clausesParam(clauses))
}
