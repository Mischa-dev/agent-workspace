// Profiles (C2.17): a title, pronouns, a time zone and an avatar for every
// account, people and agents alike, shown to those who share a workspace
// with it. Read with `profile_get`, changed with `profile_update`; the
// workspace's avatars come from `profile_avatar_list`.
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { callOp, opKey, opPrefix, opQuery, type OpInput, type OpOutput } from '../api/ops'
import { useWorkspaceId } from '../app/workspace'
import type { AccountStatus } from '../status/api'
import type { ActorKind } from '../ui'

export type Profile = OpOutput<'profile_get'>

/** The design's identity kind for an account kind (person, agent, app). */
export function profileKind(kind: string): ActorKind {
  if (kind === 'agent') return 'agent'
  if (kind === 'app' || kind === 'system') return 'app'
  return 'person'
}

/** Someone's profile as this workspace shows it; your own with no id. */
export function useProfile(accountId: string | null | undefined, enabled = true) {
  const workspaceId = useWorkspaceId() ?? undefined
  const input = accountId ? { account: accountId } : {}
  return useQuery({ ...opQuery('profile_get', input, { workspaceId }), enabled: enabled && Boolean(workspaceId), staleTime: 60_000 })
}

/** The workspace's avatars, by account id. */
export function useAvatarList(workspaceId: string | undefined) {
  return useQuery({ ...opQuery('profile_avatar_list', {}, { workspaceId }), enabled: Boolean(workspaceId), staleTime: 5 * 60_000 })
}

/** Changes a profile; the answer replaces the one on screen and avatars read again. */
export function useProfileUpdate() {
  const workspaceId = useWorkspaceId() ?? undefined
  const queryClient = useQueryClient()
  return (input: OpInput<'profile_update'>) =>
    callOp('profile_update', input, { workspaceId }).then(async (profile) => {
      queryClient.setQueryData(opKey('profile_get', input.account ? { account: input.account } : {}, workspaceId), profile)
      await queryClient.invalidateQueries({ queryKey: opPrefix('profile_get', workspaceId) })
      await queryClient.invalidateQueries({ queryKey: opPrefix('profile_avatar_list', workspaceId) })
      return profile
    })
}

/** A file as base64, for `avatarBase64`. */
export function fileBase64(file: Blob): Promise<string> {
  return file.arrayBuffer().then((buffer) => {
    let binary = ''
    const bytes = new Uint8Array(buffer)
    for (let start = 0; start < bytes.length; start += 0x8000) {
      binary += String.fromCharCode(...bytes.subarray(start, start + 0x8000))
    }
    return btoa(binary)
  })
}

/** The time it is now where someone is, such as "3:40 PM"; null without a zone. */
export function localTime(timeZone: string | null | undefined, now: Date = new Date()): string | null {
  if (!timeZone) return null
  try {
    return new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit', timeZone }).format(now)
  } catch {
    return null
  }
}

/** The status a profile shows: the account's own, then its line about work here. */
export function profileStatus(profile: Profile): AccountStatus | null {
  const line = profile.statuses.find((status) => !status.workspaceOnly) ?? profile.statuses[0]
  if (!line) return null
  return {
    accountId: line.accountId,
    workspaceOnly: line.workspaceOnly,
    emoji: line.emoji ?? undefined,
    text: line.text ?? undefined,
    state: line.state ?? undefined,
    clearAt: line.clearAt ?? undefined,
    awayUntil: line.awayUntil ?? undefined,
    updatedAt: line.updatedAt,
  }
}
