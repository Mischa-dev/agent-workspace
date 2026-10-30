import { useQuery } from '@tanstack/react-query'

import { useInbox } from '../inbox/api'
import { listUsage, usageKeys } from '../settings/account/usageApi'
import { getMyWork, viewKeys } from '../views/api'
import { getHome, homeKeys } from './homeApi'

/** Which Home sections loaded with nothing to show, so Home can say so in one line instead of four empty cards. */
export type QuietSections = { needsYou: boolean; working: boolean; capacity: boolean; tasks: boolean }

export function useQuietSections(): QuietSections {
  const inbox = useInbox('unread', 'all')
  const home = useQuery({ queryKey: homeKeys.summary, queryFn: getHome, refetchInterval: 30_000 })
  const usage = useQuery({ queryKey: usageKeys.list, queryFn: listUsage, refetchInterval: 60_000 })
  const work = useQuery({ queryKey: viewKeys.myWork, queryFn: getMyWork })
  return {
    needsYou: inbox.isSuccess && (inbox.data.pages[0]?.notifications.length ?? 0) === 0,
    working: home.isSuccess && home.data.working.length === 0 && home.data.queuedLaunches === 0,
    capacity: usage.isSuccess && usage.data.accounts.length === 0,
    tasks: work.isSuccess && work.data.claimed.items.length === 0 && work.data.assigned.items.length === 0,
  }
}
