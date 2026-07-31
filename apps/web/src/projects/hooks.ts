import {
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import {
  archiveProject,
  createProject,
  getProject,
  getProjectNavigation,
  listProjects,
} from './api'
import {
  enqueueProjectNavigationUpdate,
  enqueueProjectSwitch,
} from './navigationQueue'
import type {
  ProjectListResponse,
  ProjectNavigation,
} from './types'

export function useProjects() {
  return useQuery({
    queryKey: queryKeys.projects,
    queryFn: () => listProjects(),
    staleTime: 30_000,
  })
}

export function useProjectSearch(query: string) {
  const normalized = query.trim()
  return useQuery({
    queryKey: ['projects', 'search', normalized] as const,
    queryFn: () => listProjects(normalized),
    enabled: Boolean(normalized),
    staleTime: 15_000,
  })
}

export function useProject(projectId: string) {
  return useQuery({
    queryKey: queryKeys.project(projectId),
    queryFn: () => getProject(projectId),
    enabled: Boolean(projectId),
  })
}

export function useProjectNavigation(projectId: string) {
  return useQuery({
    queryKey: queryKeys.navigation(projectId),
    queryFn: () => getProjectNavigation(projectId),
    enabled: Boolean(projectId),
  })
}

export function useCreateProject() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: createProject,
    onSuccess: (project) => {
      queryClient.setQueryData(queryKeys.project(project.id), project)
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects })
    },
  })
}

export function useArchiveProject() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({
      projectId,
      expectedVersion,
    }: {
      projectId: string
      expectedVersion: number
    }) => archiveProject(projectId, expectedVersion),
    onSuccess: (project) => {
      queryClient.setQueryData(queryKeys.project(project.id), project)
      removeProjectFromListCaches(queryClient, project.id)
      void queryClient.invalidateQueries({ queryKey: ['projects'] })
    },
  })
}

export function removeProjectFromListCaches(
  queryClient: QueryClient,
  projectId: string,
) {
  queryClient.setQueriesData<ProjectListResponse>(
    { queryKey: ['projects'] },
    (current) =>
      current
        ? {
            ...current,
            projects: current.projects.filter(
              (project) => project.id !== projectId,
            ),
          }
        : current,
  )
}

export function useSwitchProject() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({
      projectId,
      expectedVersion,
      navigation,
    }: {
      projectId: string
      expectedVersion: number
      navigation?: ProjectNavigation
    }) =>
      enqueueProjectSwitch(queryClient, {
        projectId,
        expectedVersion,
        navigation,
      }),
  })
}

export function useUpdateProjectNavigation() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (
      input: Parameters<typeof enqueueProjectNavigationUpdate>[1],
    ) => enqueueProjectNavigationUpdate(queryClient, input),
  })
}
