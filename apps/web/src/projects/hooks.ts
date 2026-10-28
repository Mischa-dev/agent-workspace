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
  restoreProject,
  setRailOrder,
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

/** Archived projects you still belong to, most recently archived first. */
export function useArchivedProjects() {
  return useQuery({
    queryKey: ['projects', 'archived'] as const,
    queryFn: () => listProjects(undefined, { archived: true }),
    staleTime: 30_000,
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

export function useRestoreProject() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({
      projectId,
      expectedVersion,
    }: {
      projectId: string
      expectedVersion: number
    }) => restoreProject(projectId, expectedVersion),
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

/**
 * Saves your rail's order from a drag. The rail moves at once; if the save
 * fails it goes back to what the server has.
 */
export function useSetRailOrder() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (projectIds: string[]) => setRailOrder(projectIds),
    onMutate: async (projectIds) => {
      await queryClient.cancelQueries({ queryKey: queryKeys.projects })
      const place = new Map(projectIds.map((id, index) => [id, index]))
      queryClient.setQueryData<ProjectListResponse>(queryKeys.projects, (current) =>
        current
          ? {
              ...current,
              projects: current.projects.map((project) => {
                const position = place.get(project.id)
                if (position === undefined && project.navigation.railPosition == null) return project
                return {
                  ...project,
                  navigation: {
                    ...project.navigation,
                    pinned: position === undefined ? project.navigation.pinned : true,
                    railPosition: position ?? null,
                  },
                }
              }),
            }
          : current,
      )
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: ['projects'] }),
  })
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
