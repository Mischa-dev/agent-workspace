import { keepPreviousData, useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { getWorkItemBoard, getWorkItemGroups, listWorkItems, searchWorkItemReferences } from './api'
import {
  bulkUpdateWorkItems,
  createLabel,
  createMilestone,
  listLabels,
  listMilestones,
  releaseClaim,
  saveWorkflow,
  setDependencies,
  setRelation,
  setWatching,
  updateLabel,
  updateMilestone,
} from './planningApi'
import type { BoardColumnsBy, WorkItem, WorkItemFilters } from './types'

export const LIST_PAGE_SIZE = 100

/** The list, a page at a time by keyset cursor. */
export function useWorkItemPages(projectId: string, filters: WorkItemFilters, enabled = true) {
  return useInfiniteQuery({
    queryKey: queryKeys.workItemPages(projectId, filters),
    queryFn: ({ pageParam }) => listWorkItems(projectId, { ...filters, limit: LIST_PAGE_SIZE, cursor: pageParam || undefined }),
    initialPageParam: '',
    getNextPageParam: (last) => last.nextCursor ?? undefined,
    enabled: Boolean(projectId) && enabled,
    placeholderData: keepPreviousData,
  })
}

export function useWorkItemGroups(projectId: string, filters: WorkItemFilters, enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.workItemGroups(projectId, filters),
    queryFn: () => getWorkItemGroups(projectId, filters),
    enabled: Boolean(projectId) && enabled && Boolean(filters.groupBy),
    placeholderData: keepPreviousData,
  })
}

export function useBoard(projectId: string, query: WorkItemFilters & { columnsBy: BoardColumnsBy }, enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.workItemBoard(projectId, query),
    queryFn: () => getWorkItemBoard(projectId, query),
    enabled: Boolean(projectId) && enabled,
    placeholderData: keepPreviousData,
  })
}

export function useLabels(projectId: string, includeArchived = false) {
  return useQuery({
    queryKey: queryKeys.workItemLabels(projectId, includeArchived),
    queryFn: () => listLabels(projectId, includeArchived),
    enabled: Boolean(projectId),
    staleTime: 30_000,
  })
}

export function useMilestones(projectId: string, includeArchived = false) {
  return useQuery({
    queryKey: queryKeys.workItemMilestones(projectId, includeArchived),
    queryFn: () => listMilestones(projectId, includeArchived),
    enabled: Boolean(projectId),
    staleTime: 30_000,
  })
}

export function useTaskSearch(projectId: string, query: string, enabled = true) {
  const trimmed = query.trim()
  return useQuery({
    queryKey: queryKeys.workItemReferences(projectId, trimmed),
    queryFn: () => searchWorkItemReferences(projectId, trimmed),
    enabled: Boolean(projectId) && enabled,
    staleTime: 15_000,
    placeholderData: keepPreviousData,
  })
}

export function useInvalidateWorkItems() {
  const queryClient = useQueryClient()
  return (projectId: string) => queryClient.invalidateQueries({ queryKey: queryKeys.workItemsRoot(projectId) })
}

function useInvalidatingMutation<TInput extends { projectId: string }, TResult>(mutationFn: (input: TInput) => Promise<TResult>) {
  const invalidate = useInvalidateWorkItems()
  return useMutation({ mutationFn, onSettled: (_result, _error, input) => void invalidate(input.projectId) })
}

/** Writes that return the whole task: the detail cache takes the answer, everything else refetches. */
function useTaskMutation<TInput extends { projectId: string; workItemId: string }>(mutationFn: (input: TInput) => Promise<WorkItem>) {
  const queryClient = useQueryClient()
  const invalidate = useInvalidateWorkItems()
  return useMutation({
    mutationFn,
    onSuccess: (item, input) => {
      queryClient.setQueryData(queryKeys.workItem(input.projectId, input.workItemId), item)
    },
    onSettled: (_item, _error, input) => void invalidate(input.projectId),
  })
}

export function useBulkUpdate() {
  return useInvalidatingMutation(bulkUpdateWorkItems)
}

export function useCreateLabel() {
  return useInvalidatingMutation(createLabel)
}

export function useUpdateLabel() {
  return useInvalidatingMutation(updateLabel)
}

export function useCreateMilestone() {
  return useInvalidatingMutation(createMilestone)
}

export function useUpdateMilestone() {
  return useInvalidatingMutation(updateMilestone)
}

export function useSetDependencies() {
  return useTaskMutation(setDependencies)
}

export function useSetRelation() {
  return useTaskMutation(setRelation)
}

export function useReleaseClaim() {
  return useTaskMutation(releaseClaim)
}

export function useSetWatching() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: setWatching,
    onSuccess: (result, input) => {
      queryClient.setQueryData<WorkItem>(queryKeys.workItem(input.projectId, input.workItemId), (current) =>
        current ? { ...current, watching: result.watching, watchers: result.watchers } : current,
      )
    },
  })
}

export function useSaveWorkflow() {
  const queryClient = useQueryClient()
  const invalidate = useInvalidateWorkItems()
  return useMutation({
    mutationFn: saveWorkflow,
    onSuccess: (workflow, input) => {
      queryClient.setQueryData(queryKeys.workItemWorkflow(input.projectId), workflow)
      void invalidate(input.projectId)
    },
  })
}
