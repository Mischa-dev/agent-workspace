import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import {
  createWorkItem,
  getWorkItem,
  getWorkItemWorkflow,
  listWorkItems,
  searchAssignableActors,
  setWorkItemArchived,
  updateWorkItem,
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
