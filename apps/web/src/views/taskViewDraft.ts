import { clausesParam, parseClauses, type FieldDefinition } from '../fields/fieldModel'
import { listWorkItems } from '../work-items/api'
import { writeListState, type ListState } from '../work-items/listModel'

/** The list as a view would keep it: its address parameters and layout. */
export type ViewDraft = { query: Record<string, string>; layout: 'list' | 'board' }

/**
 * The view as it would be saved: filters, sort, grouping and board setup,
 * without the open task. "Me" follows whoever opens the view, so a shared
 * "assigned to me" view works for everyone: the person's own ID becomes
 * "me" in the assignee filters and in person fields' filters only. In any
 * other field (a text field holding an ID, say) the value stays as written,
 * as the server reads it.
 */
export function draftFrom(state: ListState, actorId: string, fields: readonly FieldDefinition[]): ViewDraft {
  const parameters = writeListState({ ...state, task: null, savedView: null, fields: null, view: 'list', showDone: false, lanes: 'none' })
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
  // A board's columns are its grouping; the list's grouping doesn't show there.
  if (state.view === 'board') {
    delete query.groupBy
    const columns = query.columnsBy
    delete query.columnsBy
    if (columns) query.groupBy = columns
  } else {
    delete query.columnsBy
  }
  return { query, layout: state.view }
}

/**
 * What the server makes of a draft, as a view's settings, and the Tasks
 * collection to keep it in: the one translation from the list's address to
 * a view is the server's.
 */
export async function draftConfig(projectId: string, draft: ViewDraft): Promise<{ config: unknown; collectionId: string | null }> {
  const read = await listWorkItems(projectId, { ...draft.query, limit: 1 })
  return { config: read.config ?? {}, collectionId: read.collectionId ?? null }
}
