// Saved searches and jump-to (C2.26). A saved search is a query in the
// search grammar, each account's own in each workspace: `saved_search_list`
// reads them and `saved_search_set` saves, renames, moves or removes one.
// `place_search` finds places by name for the command menu.
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { callOp, opKey, opQuery, type OpInput, type OpOutput } from '../api/ops'
import { useWorkspaceId } from '../app/workspace'

export type SavedSearch = OpOutput<'saved_search_list'>['searches'][number]
export type PlaceHit = OpOutput<'place_search'>['hits'][number]

export function useSavedSearches(enabled = true) {
  const workspaceId = useWorkspaceId() ?? undefined
  return useQuery({ ...opQuery('saved_search_list', {}, { workspaceId }), enabled: enabled && Boolean(workspaceId), staleTime: 60_000 })
}

/** Saves, changes or removes one; the answer replaces the list on screen. */
export function useSetSavedSearch() {
  const workspaceId = useWorkspaceId() ?? undefined
  const queryClient = useQueryClient()
  return (input: OpInput<'saved_search_set'>) =>
    callOp('saved_search_set', input, { workspaceId }).then((saved) => {
      queryClient.setQueryData(opKey('saved_search_list', {}, workspaceId), saved)
      return saved
    })
}

/** Places whose names match `typed`, once it is two characters or more. */
export function usePlaceSearch(typed: string, enabled = true) {
  const workspaceId = useWorkspaceId() ?? undefined
  const q = typed.trim()
  return useQuery({
    ...opQuery('place_search', { q }, { workspaceId }),
    enabled: enabled && Boolean(workspaceId) && q.length >= 2 && q.length <= 80,
    staleTime: 15_000,
    retry: false,
  })
}

/** A short name for a query, for saving it: its first 80 characters. */
export function savedSearchName(query: string): string {
  const name = query.trim().replace(/\s+/g, ' ')
  return name.length > 80 ? `${name.slice(0, 79)}…` : name
}
