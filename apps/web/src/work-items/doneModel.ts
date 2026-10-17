import { SEMANTIC_CATEGORIES, type FilterValues } from './listModel'
import type { BoardColumn, WorkItemFilters, WorkItemGroup, WorkItemWorkflow } from './types'

/**
 * Done and canceled work sits behind one "Show done" toggle on both the list
 * and the board, collapsed until someone opens it.
 */

export const DONE_CATEGORIES = ['completed', 'canceled'] as const

export function isDoneCategory(category: string | null | undefined) {
  return category === 'completed' || category === 'canceled'
}

/**
 * Whether done work goes behind the toggle. A status or status-group filter
 * already says what to show, and a view that hides done work outright
 * (includeDone=false) keeps it hidden, toggle and all.
 */
export function tucksDone(filters: FilterValues) {
  return filters.includeDone !== 'false' && !filters.statusId && !filters.semanticCategory
}

/** The filters with done work left out, for the open part of the list or board. */
export function withoutDone<T extends WorkItemFilters>(filters: T): T {
  return { ...filters, includeDone: 'false' }
}

/** The filters for counting done work by status: everything but the done rule itself. */
export function doneCountQuery(filters: FilterValues): WorkItemFilters {
  const rest: WorkItemFilters = { ...filters, groupBy: 'status' }
  delete rest.includeDone
  return rest
}

/** How many done or canceled tasks a status-grouped count holds. */
export function doneCountFromGroups(groups: readonly WorkItemGroup[] | undefined, workflow: WorkItemWorkflow): number {
  if (!groups) return 0
  const categories = new Map(workflow.statuses.map((status) => [status.id, status.semanticCategory]))
  return groups.reduce((sum, group) => (isDoneCategory(categories.get(group.key)) ? sum + group.count : sum), 0)
}

const categoryRank = (category: string | undefined) => {
  const index = category ? SEMANTIC_CATEGORIES.indexOf(category) : -1
  return index < 0 ? SEMANTIC_CATEGORIES.length : index
}

/**
 * Status columns in workflow order: by each status's position, then by status
 * group (backlog, not started, in progress, … done, canceled) where positions
 * tie. Columns the workflow doesn't know keep their order after the rest.
 */
export function orderStatusColumns<T extends Pick<BoardColumn, 'key' | 'status'>>(columns: readonly T[], workflow: WorkItemWorkflow): T[] {
  const statuses = new Map(workflow.statuses.map((status) => [status.id, status]))
  return columns
    .map((column, index) => ({ column, index, status: statuses.get(column.key) }))
    .sort((a, b) => {
      if (!a.status || !b.status) return a.status ? -1 : b.status ? 1 : a.index - b.index
      if (a.status.position !== b.status.position) return a.status.position - b.status.position
      const byCategory = categoryRank(a.status.semanticCategory) - categoryRank(b.status.semanticCategory)
      return byCategory || a.index - b.index
    })
    .map((entry) => entry.column)
}

/**
 * The status board's columns as drawn: in workflow order, with the done and
 * canceled columns left out while they're tucked away. `doneTotal` counts the
 * tasks in those columns, for the toggle.
 */
export function statusBoardColumns<T extends Pick<BoardColumn, 'key' | 'status' | 'total'>>(
  columns: readonly T[],
  workflow: WorkItemWorkflow,
  options: { tucked: boolean; showDone: boolean; hideDone: boolean },
): { visible: T[]; doneTotal: number } {
  const ordered = orderStatusColumns(columns, workflow)
  const done = (column: T) => isDoneCategory(column.status?.semanticCategory ?? workflow.statuses.find((status) => status.id === column.key)?.semanticCategory)
  const doneTotal = ordered.filter(done).reduce((sum, column) => sum + column.total, 0)
  const hide = options.hideDone || (options.tucked && !options.showDone)
  return { visible: hide ? ordered.filter((column) => !done(column)) : ordered, doneTotal }
}
