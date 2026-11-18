// Saved inbox views (C2.11): tabs a person keeps on their inbox, each a
// filter (view, category, spaces) and a density, their own in each
// workspace. `inbox_view_list` reads them; `inbox_view_set` saves, renames,
// moves or removes one. The page keeps the filter in its address, so a
// view is a set of search params, and the tab that matches them is the
// one shown as current.
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { callOp, opKey, opQuery, type OpInput, type OpOutput } from '../api/ops'
import { useWorkspaceId } from '../app/workspace'
import type { InboxCategory, InboxView } from './api'

export type InboxSavedView = OpOutput<'inbox_view_list'>['views'][number]
export type InboxDensity = InboxSavedView['density']
export type InboxFilter = { view: InboxView; category: InboxCategory; spaces: string[] }

export function useInboxViews() {
  const workspaceId = useWorkspaceId() ?? undefined
  return useQuery({ ...opQuery('inbox_view_list', {}, { workspaceId }), enabled: Boolean(workspaceId), staleTime: 60_000 })
}

/** Saves, changes or removes one; the answer replaces the tabs on screen. */
export function useSetInboxView() {
  const workspaceId = useWorkspaceId() ?? undefined
  const queryClient = useQueryClient()
  return (input: OpInput<'inbox_view_set'>) =>
    callOp('inbox_view_set', input, { workspaceId }).then((saved) => {
      queryClient.setQueryData(opKey('inbox_view_list', {}, workspaceId), saved)
      return saved
    })
}

/** The spaces in the address: `spaces=<id>,<id>`, in a stable order. */
export function spacesParam(value: string | null): string[] {
  if (!value) return []
  return [...new Set(value.split(',').map((part) => part.trim()).filter(Boolean))].sort().slice(0, 20)
}

/** The search params that show `filter`; defaults are left out. */
export function filterParams(filter: InboxFilter): URLSearchParams {
  const params = new URLSearchParams()
  if (filter.view !== 'inbox') params.set('view', filter.view)
  if (filter.category !== 'all') params.set('category', filter.category)
  const spaces = [...filter.spaces].sort()
  if (spaces.length > 0) params.set('spaces', spaces.join(','))
  return params
}

function sameFilter(a: InboxFilter, b: InboxFilter) {
  return filterParams(a).toString() === filterParams(b).toString()
}

/** The saved view whose filter is the one on screen, if any. */
export function currentView(views: readonly InboxSavedView[], filter: InboxFilter): InboxSavedView | null {
  return (
    views.find((saved) =>
      sameFilter({ view: saved.filter.view ?? 'inbox', category: saved.filter.category ?? 'all', spaces: saved.filter.spaces ?? [] }, filter),
    ) ?? null
  )
}

/** A name for the filter on screen, for saving it as a view. */
export function suggestedName(filter: InboxFilter, categoryLabel: string, viewLabel: string): string {
  const parts = [filter.category === 'all' ? null : categoryLabel, filter.view === 'inbox' ? null : viewLabel].filter(Boolean)
  return (parts.join(', ') || 'My inbox').slice(0, 40)
}
