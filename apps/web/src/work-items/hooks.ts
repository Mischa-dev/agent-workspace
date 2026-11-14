import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import {
  createWorkItem,
  createAcceptanceCriterion,
  createWorkItemComment,
  deleteWorkItemComment,
  editWorkItemComment,
  getWorkItem,
  getWorkItemWorkflow,
  listWorkItems,
  getWorkItemTimeline,
  namedAssignableActors,
  removeAcceptanceCriterion,
  searchAssignableActors,
  setWorkItemPrimaryRepository,
  setWorkItemArchived,
  setWorkItemPullRequestLink,
  setWorkItemRepositoryLink,
  setWorkItemThreadLink,
  updateWorkItem,
  updateAcceptanceCriterion,
  workWithWorker,
} from './api'
import type { WorkItemFilters } from './types'
import { listTaskFiles } from '../files/api'

export function useWorkItems(projectId: string, filters: WorkItemFilters) {
  return useQuery({
    queryKey: queryKeys.workItems(projectId, filters),
    queryFn: () => listWorkItems(projectId, filters),
    enabled: Boolean(projectId),
  })
}

/** One task. While it's on screen it refreshes every half minute, so a claim shows as it starts and ends. */
export function useWorkItem(projectId: string, workItemId: string) {
  return useQuery({
    queryKey: queryKeys.workItem(projectId, workItemId),
    queryFn: () => getWorkItem(projectId, workItemId),
    enabled: Boolean(projectId && workItemId),
    refetchInterval: 30_000,
  })
}

export function useWorkItemWorkflow(projectId: string) {
  return useQuery({
    queryKey: queryKeys.workItemWorkflow(projectId),
    queryFn: () => getWorkItemWorkflow(projectId),
    enabled: Boolean(projectId),
    staleTime: 60_000,
  })
}

/** A task's timeline, newest first, a page at a time; `fetchNextPage` reaches older entries. */
export function useWorkItemTimeline(projectId: string, workItemId: string) {
  return useInfiniteQuery({
    queryKey: queryKeys.workItemTimeline(projectId, workItemId),
    queryFn: ({ pageParam }) => getWorkItemTimeline(projectId, workItemId, pageParam),
    initialPageParam: null as string | null,
    getNextPageParam: (page) => page.nextCursor,
    enabled: Boolean(projectId && workItemId),
  })
}

/** Under the task's other queries, so they're refreshed together. */
export const taskFilesKey = (projectId: string, workItemId: string) =>
  ['project', projectId, 'work-items', 'files', workItemId] as const

/** A task's files: its own and its comments' (W20). */
export function useTaskFiles(projectId: string, workItemId: string) {
  return useQuery({
    queryKey: taskFilesKey(projectId, workItemId),
    queryFn: () => listTaskFiles(projectId, workItemId),
    enabled: Boolean(projectId && workItemId),
  })
}

export function useAssignableActors(projectId: string, query: string) {
  return useQuery({
    queryKey: queryKeys.workItemAssignees(projectId, query.trim()),
    queryFn: () => searchAssignableActors(projectId, query.trim()),
    enabled: Boolean(projectId),
    staleTime: 30_000,
  })
}

/** Names for these members, however far down a search they'd come. */
export function useNamedActors(projectId: string, ids: readonly string[]) {
  return useQuery({
    queryKey: [...queryKeys.workItemAssignees(projectId, ''), 'ids', ids.join(',')] as const,
    queryFn: () => namedAssignableActors(projectId, ids),
    enabled: Boolean(projectId) && ids.length > 0,
    staleTime: 60_000,
  })
}

function useInvalidateWorkItems() {
  const queryClient = useQueryClient()
  return (projectId: string) =>
    queryClient.invalidateQueries({ queryKey: queryKeys.workItemsRoot(projectId) })
}

export function useCreateWorkItem() {
  const invalidate = useInvalidateWorkItems()
  return useMutation({
    mutationFn: createWorkItem,
    onSuccess: (item) => void invalidate(item.projectId),
  })
}

export function useUpdateWorkItem() {
  const queryClient = useQueryClient()
  const invalidate = useInvalidateWorkItems()
  return useMutation({
    mutationFn: updateWorkItem,
    onSuccess: (item) => {
      queryClient.setQueryData(queryKeys.workItem(item.projectId, item.id), item)
      void invalidate(item.projectId)
    },
  })
}

export function useSetWorkItemArchived() {
  const queryClient = useQueryClient()
  const invalidate = useInvalidateWorkItems()
  return useMutation({
    mutationFn: setWorkItemArchived,
    onSuccess: (item) => {
      queryClient.setQueryData(queryKeys.workItem(item.projectId, item.id), item)
      void invalidate(item.projectId)
    },
  })
}

function useWorkItemChildMutation<TInput, TResult>(
  mutationFn: (input: TInput) => Promise<TResult>,
) {
  const queryClient = useQueryClient()
  const invalidate = useInvalidateWorkItems()
  return useMutation({
    mutationFn,
    onSuccess: (result, input) => {
      const scoped = input as { projectId: string; workItemId: string }
      if (
        result &&
        typeof result === 'object' &&
        'id' in result &&
        (result as { id?: unknown }).id === scoped.workItemId
      ) {
        queryClient.setQueryData(
          queryKeys.workItem(scoped.projectId, scoped.workItemId),
          result,
        )
      }
      void invalidate(scoped.projectId)
      void queryClient.invalidateQueries({
        queryKey: queryKeys.workItemTimeline(scoped.projectId, scoped.workItemId),
      })
    },
    // A failed change (often a stale version) refetches the task, so the
    // "try again" the error offers runs against what's there now.
    onError: (_error, input) => {
      const scoped = input as { projectId: string; workItemId: string }
      void queryClient.invalidateQueries({ queryKey: queryKeys.workItem(scoped.projectId, scoped.workItemId) })
    },
  })
}

export function useCreateAcceptanceCriterion() {
  return useWorkItemChildMutation(createAcceptanceCriterion)
}

export function useUpdateAcceptanceCriterion() {
  return useWorkItemChildMutation(updateAcceptanceCriterion)
}

export function useRemoveAcceptanceCriterion() {
  return useWorkItemChildMutation(removeAcceptanceCriterion)
}

export function useCreateWorkItemComment() {
  return useWorkItemChildMutation(createWorkItemComment)
}

export function useEditWorkItemComment() {
  return useWorkItemChildMutation(editWorkItemComment)
}

export function useDeleteWorkItemComment() {
  return useWorkItemChildMutation(deleteWorkItemComment)
}

export function useSetWorkItemThreadLink() {
  return useWorkItemChildMutation(setWorkItemThreadLink)
}

export function useSetWorkItemRepositoryLink() {
  return useWorkItemChildMutation(setWorkItemRepositoryLink)
}

export function useSetWorkItemPullRequestLink() {
  return useWorkItemChildMutation(setWorkItemPullRequestLink)
}

export function useSetWorkItemPrimaryRepository() {
  return useWorkItemChildMutation(setWorkItemPrimaryRepository)
}

export function useWorkWithWorker() {
  const queryClient = useQueryClient()
  const invalidate = useInvalidateWorkItems()
  return useMutation({
    mutationFn: workWithWorker,
    onSuccess: (_response, input) => {
      void invalidate(input.projectId)
      void queryClient.invalidateQueries({
        queryKey: queryKeys.workItem(input.projectId, input.workItemId),
      })
      void queryClient.invalidateQueries({
        queryKey: queryKeys.conversationSidebarRoot(input.projectId),
      })
    },
  })
}
