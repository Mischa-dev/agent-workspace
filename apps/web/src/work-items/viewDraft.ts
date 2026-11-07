import { clausesParam, parseClauses, type FieldDefinition } from '../fields/fieldModel'
import type { ViewDraft } from '../views/api'
import { writeListState, type ListState } from './listModel'

/**
 * The view as it would be saved: filters, sort, grouping and board setup,
 * without the open task. "Me" follows whoever opens the view, so a shared
 * "assigned to me" view works for everyone: the person's own ID becomes
 * "me" in the assignee filters and in person fields' filters only. In any
 * other field (a text field holding an ID, say) the value stays as written,
 * as the server reads it.
 */
export function draftFrom(
  state: ListState,
  actorId: string,
  fields: readonly FieldDefinition[],
): Omit<ViewDraft, 'name' | 'shared'> {
  const parameters = writeListState({ ...state, task: null, savedView: null, fields: null, view: 'list', showDone: false })
  const query = Object.fromEntries(parameters.entries())
  for (const key of ['assigneeActorId', 'notAssigneeActorId']) {
    if (query[key]) query[key] = query[key].split(',').map((id) => (id === actorId ? 'me' : id)).join(',')
  }
  if (query.fieldFilters) {
    const people = new Set(fields.filter((field) => field.type === 'person').map((field) => field.id))
    const clauses = parseClauses(query.fieldFilters).map((clause) =>
      clause.values && people.has(clause.field)
        ? { ...clause, values: clause.values.map((value) => (value === actorId ? 'me' : value)) }
        : clause,
    )
    query.fieldFilters = clausesParam(clauses) ?? ''
  }
  return { query, layout: state.view, fields: state.fields }
}
