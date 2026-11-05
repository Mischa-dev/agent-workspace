import { useEffect, useRef } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { isApiError } from '../api/errors'
import { normalizeAppearancePreferences, type AppearancePreferences } from './preferences'
import { useTheme } from './themeContext'

type StoredAppearance = { appearance: unknown; version: number }

const appearanceQueryKey = ['account', 'appearance'] as const

function getAppearance() {
  return requestJson<StoredAppearance>('/api/v1/account/appearance')
}

function putAppearance(appearance: AppearancePreferences, expectedVersion: number) {
  return requestJson<StoredAppearance>('/api/v1/account/appearance', {
    method: 'PUT',
    json: { appearance, expectedVersion },
  })
}

function fingerprint(preferences: AppearancePreferences) {
  return JSON.stringify(normalizeAppearancePreferences(preferences))
}

const SAVE_DELAY_MS = 800

/**
 * Keeps the account's saved appearance and this browser's in step, so a theme
 * chosen on the laptop shows on the phone. The browser keeps its cached copy
 * (public/theme-boot.js paints from it before the app loads); the account's
 * copy wins when they differ, and a change made here is saved to the account
 * shortly after. Everything is best effort: a failure leaves the local theme.
 */
export default function AppearanceSync() {
  const theme = useTheme()
  const queryClient = useQueryClient()
  const remote = useQuery({
    queryKey: appearanceQueryKey,
    queryFn: getAppearance,
    retry: false,
    staleTime: 60_000,
  })
  const version = useRef<number | null>(null)
  const synced = useRef<string | null>(null)
  const saving = useRef(false)
  const timer = useRef<number | undefined>(undefined)
  const { replaceAppearance } = theme
  const local: AppearancePreferences = {
    theme: theme.theme,
    overrides: theme.overrides,
    density: theme.density,
    textSize: theme.textSize,
    sidebarWidth: theme.sidebarWidth,
    panelWidth: theme.panelWidth,
  }
  const localPrint = fingerprint(local)
  const localRef = useRef(local)
  useEffect(() => {
    localRef.current = local
  })

  const save = useRef(async (preferences: AppearancePreferences, expectedVersion: number) => {
    saving.current = true
    try {
      const stored = await putAppearance(normalizeAppearancePreferences(preferences), expectedVersion)
      version.current = stored.version
      synced.current = fingerprint(normalizeAppearancePreferences(stored.appearance))
      queryClient.setQueryData(appearanceQueryKey, stored)
    } catch (reason) {
      // Changed on another device first: take the account's copy.
      if (isApiError(reason) && reason.status === 409) await queryClient.invalidateQueries({ queryKey: appearanceQueryKey })
    } finally {
      saving.current = false
    }
  })

  // The account's copy arrived (or changed on another device).
  useEffect(() => {
    if (!remote.data || saving.current || timer.current !== undefined) return
    version.current = remote.data.version
    if (remote.data.appearance && typeof remote.data.appearance === 'object') {
      const stored = normalizeAppearancePreferences(remote.data.appearance)
      synced.current = fingerprint(stored)
      if (synced.current !== fingerprint(localRef.current)) replaceAppearance(stored)
    } else {
      // Nothing saved yet: this browser's choice becomes the account's.
      synced.current = fingerprint(localRef.current)
      void save.current(localRef.current, remote.data.version)
    }
  }, [remote.data, replaceAppearance])

  // A change made here: save it once the person pauses.
  useEffect(() => {
    if (version.current === null || synced.current === null || localPrint === synced.current) return undefined
    window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => {
      timer.current = undefined
      if (version.current !== null) void save.current(localRef.current, version.current)
    }, SAVE_DELAY_MS)
    return () => {
      window.clearTimeout(timer.current)
      timer.current = undefined
    }
  }, [localPrint])

  return null
}
