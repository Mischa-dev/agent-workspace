// Who may message whom (C2.39). `dm_standing_get` says whether you can send
// in a DM and why not; `dm_request_respond` answers a message request;
// `account_block_set` blocks and unblocks; `account_block_list` lists who you
// blocked. Your DM setting (anyone, or ask first) is part of your profile.
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { callOp, opKey, opPrefix, opQuery, type OpOutput } from '../api/ops'
import { useWorkspaceId } from '../app/workspace'
import { directQueryKey } from './api'

export type DmStanding = OpOutput<'dm_standing_get'>
export type BlockedAccount = OpOutput<'account_block_list'>['blocked'][number]

export function useDmStanding(conversationId: string, enabled = true) {
  const workspaceId = useWorkspaceId() ?? undefined
  return useQuery({
    ...opQuery('dm_standing_get', { conversationId }, { workspaceId }),
    enabled: enabled && Boolean(workspaceId) && Boolean(conversationId),
    staleTime: 30_000,
  })
}

export function useBlockedAccounts(enabled = true) {
  const workspaceId = useWorkspaceId() ?? undefined
  return useQuery({ ...opQuery('account_block_list', {}, { workspaceId }), enabled: enabled && Boolean(workspaceId), staleTime: 60_000 })
}

/** Reads every DM's standing, your blocks and your DM list again. */
export function invalidateDmRules(queryClient: ReturnType<typeof useQueryClient>, workspaceId?: string) {
  void queryClient.invalidateQueries({ queryKey: opPrefix('dm_standing_get', workspaceId) })
  void queryClient.invalidateQueries({ queryKey: opPrefix('account_block_list', workspaceId) })
  void queryClient.invalidateQueries({ queryKey: opPrefix('sidebar_list', workspaceId) })
  void queryClient.invalidateQueries({ queryKey: directQueryKey })
}

/** Blocks or unblocks someone; the answer replaces your list of blocks. */
export function useSetBlock() {
  const workspaceId = useWorkspaceId() ?? undefined
  const queryClient = useQueryClient()
  return (account: string, blocked: boolean) =>
    callOp('account_block_set', { account, blocked }, { workspaceId }).then((blocks) => {
      queryClient.setQueryData(opKey('account_block_list', {}, workspaceId), blocks)
      invalidateDmRules(queryClient, workspaceId)
      return blocks
    })
}

/** Accepts or declines the message request in a DM. */
export function useAnswerRequest() {
  const workspaceId = useWorkspaceId() ?? undefined
  const queryClient = useQueryClient()
  return (conversationId: string, accept: boolean) =>
    callOp('dm_request_respond', { conversationId, accept }, { workspaceId }).then((standing) => {
      queryClient.setQueryData(opKey('dm_standing_get', { conversationId }, workspaceId), standing)
      invalidateDmRules(queryClient, workspaceId)
      return standing
    })
}

/** The note under a DM you can't send in, by the reason the server gives. */
export function standingNote(standing: DmStanding, name: string): string | null {
  switch (standing.reason) {
    case 'blocked':
      return `You blocked ${name}. Unblock them to send messages here.`
    case 'not_shared':
      return `You no longer share a workspace with ${name}, so new messages can’t be sent here.`
    case 'waiting':
      return `Your message request is waiting for ${name}. Nothing more is sent until they accept it.`
    case 'closed':
      return 'New messages can’t be sent in this conversation.'
    default:
      return null
  }
}
