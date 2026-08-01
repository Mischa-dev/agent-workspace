import type { QueryClient } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { currentQueryScopeGeneration } from '../api/queryScope'
import {
  getProjectNavigation,
  switchProject,
  updateProjectNavigation,
} from './api'
import { locationsEqual } from './locations'
import type {
  Project,
  ProjectListResponse,
  ProjectLocation,
  ProjectNavigation,
  ProjectSwitchResponse,
} from './types'

type NavigationPatch = {
  location?: ProjectLocation
  pinned?: boolean
  contextDrawerOpen?: boolean
  filters?: Record<string, unknown>
}

type NavigationMutationContext = {
  expectedVersion: number
  navigation?: ProjectNavigation
  projectId: string
}

const projectMutationQueues = new Map<string, Promise<void>>()

function isProjectListQueryKey(queryKey: readonly unknown[]) {
  return (
    queryKey[0] === 'projects' &&
    (queryKey[1] === 'rail' || queryKey[1] === 'search')
  )
}

function enqueueProjectMutation<T>(
  projectId: string,
  task: () => Promise<T>,
): Promise<T> {
  const generation = currentQueryScopeGeneration()
  const queueKey = `${generation}:${projectId}`
  const previous = projectMutationQueues.get(queueKey) ?? Promise.resolve()
  const operation = previous.catch(() => undefined).then(async () => {
    assertCurrentQueryScope(generation)
    const result = await task()
    assertCurrentQueryScope(generation)
    return result
  })
  const tail = operation.then(
    () => undefined,
    () => undefined,
  )
  projectMutationQueues.set(queueKey, tail)
  void tail.finally(() => {
    if (projectMutationQueues.get(queueKey) === tail) {
      projectMutationQueues.delete(queueKey)
    }
  })
  return operation
}

function assertCurrentQueryScope(generation: number) {
  if (generation !== currentQueryScopeGeneration()) {
    throw new Error('Navigation update cancelled after the session changed.')
  }
}

function newerNavigation(
  left: ProjectNavigation | undefined,
  right: ProjectNavigation | undefined,
) {
  if (!left) return right
  if (!right) return left
  return right.version >= left.version ? right : left
}

function cachedNavigation(
  queryClient: QueryClient,
  projectId: string,
  initial?: ProjectNavigation,
) {
  let current = initial
  current = newerNavigation(
    current,
    queryClient.getQueryData<ProjectNavigation>(
      queryKeys.navigation(projectId),
    ),
  )
  current = newerNavigation(
    current,
    queryClient.getQueryData<Project>(queryKeys.project(projectId))?.navigation,
  )
  for (const [, list] of queryClient.getQueriesData<ProjectListResponse>({
    predicate: (query) => isProjectListQueryKey(query.queryKey),
  })) {
    current = newerNavigation(
      current,
      list?.projects.find((project) => project.id === projectId)?.navigation,
    )
  }
  return current
}

function mergeNavigation(
  current: ProjectNavigation,
  incoming: ProjectNavigation,
) {
  return incoming.version >= current.version ? incoming : current
}

function installNavigation(
  queryClient: QueryClient,
  projectId: string,
  navigation: ProjectNavigation,
  project?: Project,
) {
  queryClient.setQueryData<ProjectNavigation>(
    queryKeys.navigation(projectId),
    (current) =>
      current ? mergeNavigation(current, navigation) : navigation,
  )
  queryClient.setQueryData<Project>(
    queryKeys.project(projectId),
    (current) => {
      if (!current) return project ? { ...project, navigation } : current
      const nextProject =
        project && project.version >= current.version
          ? { ...current, ...project }
          : current
      return {
        ...nextProject,
        navigation: mergeNavigation(current.navigation, navigation),
      }
    },
  )
  queryClient.setQueriesData<ProjectListResponse>(
    { predicate: (query) => isProjectListQueryKey(query.queryKey) },
    (current) => {
      if (!current) return current
      return {
        ...current,
        projects: current.projects.map((candidate) => {
          if (candidate.id !== projectId) return candidate
          const nextProject =
            project && project.version >= candidate.version
              ? { ...candidate, ...project }
              : candidate
          return {
            ...nextProject,
            navigation: mergeNavigation(candidate.navigation, navigation),
          }
        }),
      }
    },
  )
  void queryClient.invalidateQueries({
    predicate: (query) => isProjectListQueryKey(query.queryKey),
    refetchType: 'none',
  })
}

async function prepareNavigation(
  queryClient: QueryClient,
  context: NavigationMutationContext,
) {
  const generation = currentQueryScopeGeneration()
  const activeProjectListQueries = queryClient
    .getQueryCache()
    .findAll({
      predicate: (query) => isProjectListQueryKey(query.queryKey),
      type: 'active',
    })
    .map((query) => query.queryKey)
  await Promise.all([
    queryClient.cancelQueries({
      predicate: (query) => isProjectListQueryKey(query.queryKey),
    }),
    queryClient.cancelQueries({
      queryKey: queryKeys.project(context.projectId),
      exact: true,
    }),
    queryClient.cancelQueries({
      queryKey: queryKeys.navigation(context.projectId),
      exact: true,
    }),
  ])
  const known = cachedNavigation(
    queryClient,
    context.projectId,
    context.navigation,
  )
  const authoritativeNavigation = getProjectNavigation(context.projectId).then(
    (current) => {
      assertCurrentQueryScope(generation)
      const authoritative =
        known &&
        known.version >= Math.max(0, context.expectedVersion) &&
        known.version > current.version
          ? known
          : current
      installNavigation(queryClient, context.projectId, authoritative)
      return authoritative
    },
  )

  // Cancellation protects list caches from an older navigation response, but
  // a cold read has no data to restore and otherwise remains pending/idle.
  // Restart each active rail/search query independently. Reapply the
  // authoritative navigation after that exact read settles so one stalled
  // search cannot delay the mutation or another list query's recovery.
  for (const queryKey of activeProjectListQueries) {
    void queryClient
      .refetchQueries({ queryKey, exact: true, type: 'active' })
      .then(async () => {
        const prepared = await authoritativeNavigation
        assertCurrentQueryScope(generation)
        const authoritative =
          cachedNavigation(queryClient, context.projectId, prepared) ?? prepared
        installNavigation(
          queryClient,
          context.projectId,
          authoritative,
        )
      })
      .catch(() => undefined)
  }

  return authoritativeNavigation
}

async function retryAfterConflict<T>(
  queryClient: QueryClient,
  projectId: string,
  operation: (current: ProjectNavigation) => Promise<T>,
  current: ProjectNavigation,
  generation: number,
) {
  try {
    const result = await operation(current)
    assertCurrentQueryScope(generation)
    return result
  } catch (error) {
    assertCurrentQueryScope(generation)
    if (!isApiError(error) || error.status !== 409) throw error
    const refreshed = await getProjectNavigation(projectId)
    assertCurrentQueryScope(generation)
    installNavigation(queryClient, projectId, refreshed)
    const result = await operation(refreshed)
    assertCurrentQueryScope(generation)
    return result
  }
}

function patchAlreadyApplied(
  current: ProjectNavigation,
  patch: NavigationPatch,
) {
  if (patch.location && !locationsEqual(current.location, patch.location)) {
    return false
  }
  if (patch.pinned !== undefined && current.pinned !== patch.pinned) {
    return false
  }
  if (
    patch.contextDrawerOpen !== undefined &&
    current.contextDrawerOpen !== patch.contextDrawerOpen
  ) {
    return false
  }
  if (patch.filters !== undefined) return false
  return true
}

export function enqueueProjectNavigationUpdate(
  queryClient: QueryClient,
  context: NavigationMutationContext & NavigationPatch,
): Promise<ProjectNavigation> {
  return enqueueProjectMutation(context.projectId, async () => {
    const generation = currentQueryScopeGeneration()
    const current = await prepareNavigation(queryClient, context)
    assertCurrentQueryScope(generation)
    if (patchAlreadyApplied(current, context)) return current
    const update = async (latest: ProjectNavigation) =>
      updateProjectNavigation(context.projectId, {
        expectedVersion: latest.version,
        location: context.location ?? latest.location,
        ...(context.pinned === undefined ? {} : { pinned: context.pinned }),
        ...(context.contextDrawerOpen === undefined
          ? {}
          : { contextDrawerOpen: context.contextDrawerOpen }),
        ...(context.filters === undefined ? {} : { filters: context.filters }),
      })
    const navigation = await retryAfterConflict(
      queryClient,
      context.projectId,
      update,
      current,
      generation,
    )
    assertCurrentQueryScope(generation)
    installNavigation(queryClient, context.projectId, navigation)
    return navigation
  })
}

export function enqueueProjectSwitch(
  queryClient: QueryClient,
  context: NavigationMutationContext,
): Promise<ProjectSwitchResponse> {
  return enqueueProjectMutation(context.projectId, async () => {
    const generation = currentQueryScopeGeneration()
    const current = await prepareNavigation(queryClient, context)
    assertCurrentQueryScope(generation)
    const performSwitch = (latest: ProjectNavigation) =>
      switchProject(context.projectId, latest.version)
    const result = await retryAfterConflict(
      queryClient,
      context.projectId,
      performSwitch,
      current,
      generation,
    )
    assertCurrentQueryScope(generation)
    installNavigation(
      queryClient,
      context.projectId,
      result.navigation,
      result.project,
    )
    return result
  })
}
