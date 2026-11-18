import { useQuery } from '@tanstack/react-query'

import { useWorkspaceId } from '../app/workspace'
import { opQuery } from '../api/ops'

/** A2.18: agents and apps the caller manages in other workspaces. */
export function useJoinableAccounts() {
  const workspaceId = useWorkspaceId() ?? undefined
  return useQuery(opQuery('account_joinable_list', {}, { workspaceId })).data?.accounts ?? []
}

/** A2.20: the offers of the hosts the caller may use here. */
export function useHostOffers() {
  const workspaceId = useWorkspaceId() ?? undefined
  return useQuery(opQuery('agent_offer_list', {}, { workspaceId })).data?.offers ?? []
}
