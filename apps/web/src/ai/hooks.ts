import { useEffect, useRef } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import {
  aiKeys,
  analyzeProject,
  commitPlan,
  createPlan,
  decideSuggestion,
  draftCriteria,
  getGraph,
  getPlan,
  getProjectAi,
  getRanking,
  getReadiness,
  listPlans,
  listSuggestions,
  savePlan,
  triageTask,
  type Draft,
  type GraphFilters,
  type PlanImport,
  type PlanSource,
  type RankFilters,
  type SuggestionFilters,
} from './api'

const busy = (state: string) => state === 'queued' || state === 'running'

export function useProjectAi(projectId: string) {
  const queryClient = useQueryClient()
  const status = useQuery({
    queryKey: aiKeys.status(projectId),
    queryFn: () => getProjectAi(projectId),
    enabled: Boolean(projectId),
    // While a run is going, check back so its result shows up on its own.
    refetchInterval: (query) => (query.state.data?.runs.some((run) => busy(run.state)) ? 3_000 : false),
  })
  const running = status.data?.runs.some((run) => busy(run.state)) ?? false
  const wasRunning = useRef(running)
  useEffect(() => {
    // A run just finished: show what it found (and anything it applied).
    if (wasRunning.current && !running) {
      void queryClient.invalidateQueries({
        predicate: (query) => query.queryKey[0] === 'project' && query.queryKey[1] === projectId && query.queryKey[2] === 'ai' && query.queryKey[3] !== 'status',
      })
      void queryClient.invalidateQueries({ queryKey: queryKeys.workItemsRoot(projectId) })
    }
    wasRunning.current = running
  }, [projectId, queryClient, running])
  return status
}

export function useSuggestions(projectId: string, filters: SuggestionFilters = {}, enabled = true) {
  return useQuery({
    queryKey: aiKeys.suggestions(projectId, filters),
    queryFn: () => listSuggestions(projectId, filters),
    enabled: Boolean(projectId) && enabled,
  })
}

export function useRanking(projectId: string, filters: RankFilters = {}) {
  return useQuery({
    queryKey: aiKeys.ranking(projectId, filters),
    queryFn: () => getRanking(projectId, filters),
    enabled: Boolean(projectId),
  })
}

export function useGraph(projectId: string, filters: GraphFilters = {}) {
  return useQuery({
    queryKey: aiKeys.graph(projectId, filters),
    queryFn: () => getGraph(projectId, filters),
    enabled: Boolean(projectId),
  })
}

export function useReadiness(projectId: string, workItemId: string, version: number) {
  return useQuery({
    // The task's version is in the key so an edit rechecks it.
    queryKey: [...aiKeys.readiness(projectId, workItemId), version],
    queryFn: () => getReadiness(projectId, workItemId),
    enabled: Boolean(projectId && workItemId),
  })
}

export function usePlans(projectId: string) {
  return useQuery({
    queryKey: aiKeys.plans(projectId),
    queryFn: () => listPlans(projectId),
    enabled: Boolean(projectId),
  })
}

export function usePlan(projectId: string, importId: string | null) {
  return useQuery({
    queryKey: aiKeys.plan(projectId, importId ?? ''),
    queryFn: () => getPlan(projectId, importId ?? ''),
    enabled: Boolean(projectId && importId),
    refetchInterval: (query) => {
      const state = query.state.data?.plan.state
      return state === 'drafting' || state === 'committing' ? 2_000 : false
    },
  })
}

/** Refresh AI views and the tasks a decision may have changed. */
function useRefresh(projectId: string) {
  const queryClient = useQueryClient()
  return () =>
    Promise.all([
      queryClient.invalidateQueries({ queryKey: aiKeys.root(projectId) }),
      queryClient.invalidateQueries({ queryKey: queryKeys.workItemsRoot(projectId) }),
    ])
}

export function useDecideSuggestion(projectId: string) {
  const refresh = useRefresh(projectId)
  return useMutation({
    mutationFn: ({ suggestionId, decision }: { suggestionId: string; decision: 'accept' | 'dismiss' }) =>
      decideSuggestion(projectId, suggestionId, decision),
    onSettled: refresh,
  })
}

export function useAnalyze(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () => analyzeProject(projectId),
    onSettled: () => queryClient.invalidateQueries({ queryKey: aiKeys.status(projectId) }),
  })
}

export function useTriage(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (workItemId: string) => triageTask(projectId, workItemId),
    onSettled: () => queryClient.invalidateQueries({ queryKey: aiKeys.status(projectId) }),
  })
}

export function useDraftCriteria(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (workItemId: string) => draftCriteria(projectId, workItemId),
    onSettled: () => queryClient.invalidateQueries({ queryKey: aiKeys.root(projectId) }),
  })
}

export function useCreatePlan(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ title, source }: { title: string; source: PlanSource }) => createPlan(projectId, title, source),
    onSuccess: ({ plan }) => queryClient.setQueryData(aiKeys.plan(projectId, plan.id), (old: { canEdit: boolean } | undefined) => ({ plan, canEdit: old?.canEdit ?? true })),
    onSettled: () => queryClient.invalidateQueries({ queryKey: aiKeys.plans(projectId) }),
  })
}

export function useSavePlan(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ plan, draft, title }: { plan: PlanImport; draft: Draft; title?: string }) => savePlan(projectId, plan, draft, title),
    onSuccess: ({ plan }) =>
      queryClient.setQueryData(aiKeys.plan(projectId, plan.id), (old: { canEdit: boolean } | undefined) => ({ plan, canEdit: old?.canEdit ?? true })),
  })
}

export function useCommitPlan(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (plan: PlanImport) => commitPlan(projectId, plan),
    onSuccess: ({ plan }) =>
      queryClient.setQueryData(aiKeys.plan(projectId, plan.id), (old: { canEdit: boolean } | undefined) => ({ plan, canEdit: old?.canEdit ?? true })),
    onSettled: () => queryClient.invalidateQueries({ queryKey: aiKeys.plans(projectId) }),
  })
}
