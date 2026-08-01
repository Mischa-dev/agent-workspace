import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { enqueueProjectNavigationUpdate } from './navigationQueue'
import type { ProjectLocation, ProjectNavigation } from './types'

function locationKey(location: ProjectLocation) {
  return location.kind === 'thread' || location.kind === 'channel'
    ? `${location.kind}:${location.conversationId}`
    : location.kind
}

export function useRememberProjectLocation(
  projectId: string,
  location: ProjectLocation,
  enabled: boolean,
  navigation?: ProjectNavigation,
  filters?: Record<string, unknown>,
) {
  const queryClient = useQueryClient()
  const desiredLocationKey = locationKey(location)
  const syncKey = `${projectId}:${desiredLocationKey}`
  const filtersKey = JSON.stringify(filters ?? null)
  const [retryCount, setRetryCount] = useState(0)
  const [failure, setFailure] = useState<{
    key: string
    message: string
  } | null>(null)

  useEffect(() => {
    if (!enabled || !projectId) return
    let active = true
    void enqueueProjectNavigationUpdate(queryClient, {
      projectId,
      location,
      expectedVersion: navigation?.version ?? 0,
      navigation,
      filters,
    })
      .then(() => {
        if (!active) return
        setFailure((current) => (current?.key === syncKey ? null : current))
      })
      .catch((reason) => {
        if (!active) return
        setFailure({
          key: syncKey,
          message: isApiError(reason)
            ? reason.message
            : 'Your recent Project location could not be saved.',
        })
      })
    return () => {
      active = false
    }
    // `desiredLocationKey` makes this effect stable across equivalent object values.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    desiredLocationKey,
    enabled,
    filtersKey,
    projectId,
    queryClient,
    retryCount,
  ])

  return {
    error: failure?.key === syncKey ? failure.message : null,
    retry: () => {
      setFailure(null)
      setRetryCount((count) => count + 1)
    },
  }
}
