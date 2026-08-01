import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import {
  createWorkItem,
  createAcceptanceCriterion,
  createWorkItemComment,
  getWorkItem,
  getWorkItemWorkflow,
  listWorkItems,
  getWorkItemTimeline,
  removeAcceptanceCriterion,
  searchAssignableActors,
  setWorkItemPrimaryRepository,
  setWorkItemArchived,
  setWorkItemRepositoryLink,
  setWorkItemThreadLink,
  updateWorkItem,
  updateAcceptanceCriterion,
} from './api'
import type { WorkItemFilters } from './types'

export function useWorkItems(projectId: string, filters: WorkItemFilters) {
  return useQuery({
    queryKey: queryKeys.workItems(projectId, filters),
    queryFn: () => listWorkItems(projectId, filters),
    enabled: Boolean(projectId),
  })
}

export function useWorkItem(projectId: string, workItemId: string) {
  return useQuery({
    queryKey: queryKeys.workItem(projectId, workItemId),
    queryFn: () => getWorkItem(projectId, workItemId),
    enabled: Boolean(projectId && workItemId),
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

export function useWorkItemTimeline(projectId: string, workItemId: string) {
  return useQuery({
    queryKey: queryKeys.workItemTimeline(projectId, workItemId),
    queryFn: () => getWorkItemTimeline(projectId, workItemId),
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

export function useSetWorkItemThreadLink() {
  return useWorkItemChildMutation(setWorkItemThreadLink)
}

export function useSetWorkItemRepositoryLink() {
  return useWorkItemChildMutation(setWorkItemRepositoryLink)
}

export function useSetWorkItemPrimaryRepository() {
  return useWorkItemChildMutation(setWorkItemPrimaryRepository)
}
