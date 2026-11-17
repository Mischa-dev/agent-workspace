// Statuses (C2.15): the line and emoji next to a name, for people and agents
// alike. Set and cleared through the `status_set` and `status_delete`
// operations; shown from `status_list`, which answers only for accounts in
// the workspace asked about.
import { useQuery } from '@tanstack/react-query'

import { callOp, newRequestId } from '../api/ops'
import { queryKeys } from '../api/queryKeys'

export type AccountStatus = {
  accountId: string
  /** A line about work in this workspace only, not the account's own status. */
  workspaceOnly: boolean
  emoji?: string
  text?: string
  state?: string
  clearAt?: string
  awayUntil?: string
  updatedAt: string
}

export const statusKeys = {
  all: queryKeys.statuses,
  list: (workspaceId: string, accountIds: readonly string[]) => [...queryKeys.statuses, workspaceId, [...accountIds].sort().join(',')] as const,
}

export function setStatus(
  workspaceId: string,
  input: { text?: string; emoji?: string; clearAt?: string },
  requestId: string = newRequestId(),
) {
  return callOp('status_set', input, { workspaceId, requestId }) as Promise<AccountStatus>
}

export function clearStatus(workspaceId: string, requestId: string = newRequestId()) {
  return callOp('status_delete', {}, { workspaceId, requestId })
}

export function listStatuses(workspaceId: string, accountIds: readonly string[]) {
  return callOp('status_list', { accounts: [...accountIds] }, { workspaceId }).then(
    (output) => output.statuses as AccountStatus[],
  )
}

/** What shows for an account: its own status, then its line about work here. */
export function shownStatus(statuses: readonly AccountStatus[], accountId: string): AccountStatus | null {
  const mine = statuses.filter((status) => status.accountId === accountId)
  return mine.find((status) => !status.workspaceOnly) ?? mine[0] ?? null
}

/** Statuses for these accounts in a workspace, fetched together. */
export function useStatuses(workspaceId: string | null, accountIds: readonly string[]) {
  const ids = [...new Set(accountIds.filter(Boolean))].slice(0, 200)
  return useQuery({
    queryKey: statusKeys.list(workspaceId ?? '', ids),
    queryFn: () => listStatuses(workspaceId ?? '', ids),
    enabled: Boolean(workspaceId) && ids.length > 0,
    staleTime: 60_000,
  })
}
