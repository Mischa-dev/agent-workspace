import { useCallback, useMemo } from 'react'

import { doneCountFromGroups, doneCountQuery } from './doneModel'
import type { FilterValues } from './listModel'
import { useWorkItemGroups, useWorkItemPages } from './planningHooks'
import type { WorkItemFilters, WorkItemWorkflow } from './types'

/** How many done and canceled tasks match the filters, for the Show done toggle. */
export function useDoneCount(projectId: string, filters: FilterValues, workflow: WorkItemWorkflow, enabled: boolean) {
  const query = useMemo(() => doneCountQuery(filters), [filters])
  const groups = useWorkItemGroups(projectId, query, enabled)
  return enabled ? doneCountFromGroups(groups.data, workflow) : 0
}

/**
 * The done section of the list: done tasks, then canceled ones, in the list's
 * sort, loaded page by page once the section is open.
 */
export function useDoneItems(projectId: string, query: WorkItemFilters, open: boolean) {
  const base = useMemo(() => {
    const rest: WorkItemFilters = { ...query }
    delete rest.groupBy
    delete rest.includeDone
    return rest
  }, [query])
  const completedQuery = useMemo(() => ({ ...base, semanticCategory: 'completed' }), [base])
  const canceledQuery = useMemo(() => ({ ...base, semanticCategory: 'canceled' }), [base])
  const completed = useWorkItemPages(projectId, completedQuery, open)
  const canceled = useWorkItemPages(projectId, canceledQuery, open)

  const completedItems = useMemo(() => completed.data?.pages.flatMap((page) => page.items) ?? [], [completed.data])
  const canceledItems = useMemo(() => canceled.data?.pages.flatMap((page) => page.items) ?? [], [canceled.data])
  // Canceled tasks follow once every done one has loaded, so the section reads in one order.
  const completedDone = Boolean(completed.data) && !completed.hasNextPage
  const items = useMemo(
    () => (!open ? [] : completedDone ? [...completedItems, ...canceledItems] : completedItems),
    [canceledItems, completedDone, completedItems, open],
  )
  const hasMore = open && (!completedDone || !canceled.data || Boolean(canceled.hasNextPage))
  const current = completedDone ? canceled : completed
  const { fetchNextPage, hasNextPage, isFetchingNextPage, isFetchNextPageError, isError } = current
  const fetchMore = useCallback(() => {
    if (hasNextPage && !isFetchingNextPage && !isFetchNextPageError) void fetchNextPage()
  }, [fetchNextPage, hasNextPage, isFetchNextPageError, isFetchingNextPage])
  return { items, hasMore, fetchMore, failed: isFetchNextPageError || isError, retry: () => void (isError ? current.refetch() : fetchNextPage()) }
}
