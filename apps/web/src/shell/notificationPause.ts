import { useEffect, useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { getNotificationSettings, notificationKeys, setPersonalSetting } from '../settings/account/usageApi'

/**
 * Pausing notifications by hand ("for an hour", "until tomorrow"). The pause
 * is a personal setting, `notifications.paused_until`, in seconds; the server
 * holds pushes until it ends and skips emails, and the inbox keeps everything.
 */

export type PauseChoice = { id: string; label: string; until: Date }

/** Tomorrow at 9:00 here, or today at 9:00 if it's still the small hours. */
function nextMorning(now: Date) {
  const morning = new Date(now)
  morning.setHours(9, 0, 0, 0)
  if (now.getHours() >= 5) morning.setDate(morning.getDate() + 1)
  return morning
}

export function pauseChoices(now: Date): PauseChoice[] {
  return [
    { id: '1h', label: 'For 1 hour', until: new Date(now.getTime() + 60 * 60_000) },
    { id: '4h', label: 'For 4 hours', until: new Date(now.getTime() + 4 * 60 * 60_000) },
    { id: 'morning', label: now.getHours() >= 5 ? 'Until tomorrow morning' : 'Until this morning', until: nextMorning(now) },
  ]
}

/** "14:30", or "tomorrow 09:00" when it isn't today. */
export function pausedUntilLabel(until: Date, now: Date) {
  const time = until.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
  if (until.toDateString() === now.toDateString()) return time
  const tomorrow = new Date(now)
  tomorrow.setDate(now.getDate() + 1)
  if (until.toDateString() === tomorrow.toDateString()) return `tomorrow ${time}`
  return `${until.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' })} ${time}`
}

/** When the pause ends, if one is on. Re-renders the moment it runs out. */
export function useNotificationPause() {
  const queryClient = useQueryClient()
  const settings = useQuery({
    queryKey: notificationKeys.settings,
    queryFn: getNotificationSettings,
    staleTime: 60_000,
    retry: false,
  })
  const stored = settings.data?.quietHours.pausedUntil
  const ends = stored ? Date.parse(stored) : Number.NaN
  const [now, setNow] = useState(() => Date.now())
  const until = useMemo(() => (ends > now ? new Date(ends) : null), [ends, now])

  useEffect(() => {
    if (!until) return undefined
    const timer = window.setTimeout(() => setNow(Date.now()), Math.min(until.getTime() - Date.now() + 500, 2 ** 31 - 1))
    return () => window.clearTimeout(timer)
  }, [until])

  const change = useMutation({
    mutationFn: async (next: Date | null) => {
      if (!settings.data) throw new Error('Notification settings haven’t loaded yet.')
      await setPersonalSetting(settings.data.organizationId, 'notifications.paused_until', next ? Math.floor(next.getTime() / 1000) : 0)
    },
    onSuccess: () => {
      setNow(Date.now())
      void queryClient.invalidateQueries({ queryKey: notificationKeys.settings })
    },
  })

  return {
    available: Boolean(settings.data),
    until,
    pause: (next: Date) => change.mutateAsync(next),
    resume: () => change.mutateAsync(null),
  }
}
