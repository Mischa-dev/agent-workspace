import { useCallback, useMemo, type ReactNode } from 'react'

import { AvatarSources } from '../ui'
import { useAvatarList } from './api'

/**
 * Gives every avatar in a workspace its picture (C2.17): one list of the
 * workspace's avatars, looked up by account id, so a timeline, a member list
 * or a picker shows them without carrying the address itself.
 */
export function AvatarSourcesProvider({ children, workspaceId }: { children: ReactNode; workspaceId: string | undefined }) {
  const list = useAvatarList(workspaceId)
  const byAccount = useMemo(() => new Map((list.data?.avatars ?? []).map((line) => [line.accountId, line.url])), [list.data])
  const lookup = useCallback((accountId: string) => byAccount.get(accountId), [byAccount])
  return <AvatarSources.Provider value={lookup}>{children}</AvatarSources.Provider>
}
