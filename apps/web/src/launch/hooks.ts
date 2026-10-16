import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import {
  cancelLaunch,
  checkEnvironment,
  createEnvironment,
  createLaunches,
  environmentAction,
  getLaunchPlan,
  getProjectSettings,
  listAgentSessions,
  listEnvironments,
  listLaunches,
  resetProjectSetting,
  setProjectSetting,
  updateEnvironment,
  type EnvironmentInput,
  type Launch,
  type LaunchInput,
} from './api'

/** Launches waiting or just started change on the server without an event, so look again while any are in flight. */
const LIVE_REFRESH_MS = 10_000

function inFlight(launches: Launch[]) {
  return launches.some((launch) => launch.state === 'queued' || (launch.state === 'started' && isActiveRun(launch.runState)))
}

export function isActiveRun(state: string | null) {
  return state !== null && ['created', 'queued', 'starting', 'working', 'blocked', 'recovering'].includes(state)
}

export function useLaunchPlan(projectId: string, workItemId: string, choice: { environmentId?: string; harness?: string } = {}, enabled = true) {
  return useQuery({
    queryKey: queryKeys.launchPlan(projectId, workItemId, choice),
    queryFn: () => getLaunchPlan(projectId, workItemId, choice),
    enabled: enabled && Boolean(projectId && workItemId),
    refetchInterval: (query) => (query.state.data && inFlight(query.state.data.launches) ? LIVE_REFRESH_MS : false),
  })
}

export function useLaunchList(projectId: string) {
  return useQuery({
    queryKey: queryKeys.launches(projectId),
    queryFn: () => listLaunches(projectId),
    enabled: Boolean(projectId),
    refetchInterval: (query) =>
      query.state.data && (query.state.data.queued.length > 0 || inFlight(query.state.data.recent)) ? LIVE_REFRESH_MS : false,
  })
}

export function useCreateLaunches(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ input, idempotencyKey }: { input: LaunchInput; idempotencyKey: string }) =>
      createLaunches(projectId, input, idempotencyKey),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.launches(projectId) })
      void queryClient.invalidateQueries({ queryKey: queryKeys.conversationSidebarRoot(projectId) })
    },
  })
}

export function useCancelLaunch(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (launchId: string) => cancelLaunch(projectId, launchId),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: queryKeys.launches(projectId) }),
  })
}

export function useEnvironments(projectId: string) {
  return useQuery({
    queryKey: queryKeys.environments(projectId),
    queryFn: () => listEnvironments(projectId),
    enabled: Boolean(projectId),
  })
}

export function useSaveEnvironment(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ environmentId, input }: { environmentId?: string; input: EnvironmentInput }) =>
      environmentId ? updateEnvironment(projectId, environmentId, input) : createEnvironment(projectId, input),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.environments(projectId) })
      void queryClient.invalidateQueries({ queryKey: queryKeys.launches(projectId) })
    },
  })
}

export function useEnvironmentAction(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async ({
      environmentId,
      action,
    }: {
      environmentId: string
      action: 'archive' | 'default' | 'check'
    }): Promise<{ ok: boolean; message: string } | null> => {
      if (action === 'check') return checkEnvironment(projectId, environmentId)
      await environmentAction(projectId, environmentId, action)
      return null
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.environments(projectId) })
      void queryClient.invalidateQueries({ queryKey: queryKeys.launches(projectId) })
    },
  })
}

export function useAgentSessions(projectId: string, conversationId: string, enabled = true) {
  return useQuery({
    queryKey: queryKeys.agentSessions(projectId, conversationId),
    queryFn: () => listAgentSessions(conversationId),
    enabled: enabled && Boolean(conversationId),
    refetchInterval: (query) =>
      query.state.data?.sessions.some((session) => session.state === 'active') ? LIVE_REFRESH_MS * 3 : false,
  })
}

export function useProjectSettings(projectId: string) {
  return useQuery({
    queryKey: queryKeys.projectSettings(projectId),
    queryFn: () => getProjectSettings(projectId),
    enabled: Boolean(projectId),
  })
}

export function useSaveProjectSetting(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ key, value }: { key: string; value: unknown | undefined }) =>
      value === undefined ? resetProjectSetting(projectId, key) : setProjectSetting(projectId, key, value),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.projectSettings(projectId) })
      void queryClient.invalidateQueries({ queryKey: queryKeys.launches(projectId) })
    },
  })
}
