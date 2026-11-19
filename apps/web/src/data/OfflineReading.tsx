import { useEffect } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { useEffectiveSettings } from '../settings/api'
import { offlineOwner, offlineReadingOn } from './offlinePointer'
import { clearOffline, keepOffline } from './persist'

function Keeper({ actorId, workspaceId }: { actorId: string; workspaceId: string }) {
  const queryClient = useQueryClient()
  const effective = useEffectiveSettings({ scope: 'workspace', organizationId: workspaceId })
  // Unknown (offline, or still loading) changes nothing.
  const on = effective.data ? offlineReadingOn(effective.data) : null
  useEffect(() => {
    if (on === null) return
    if (!on) {
      if (offlineOwner()) void clearOffline()
      return
    }
    return keepOffline(queryClient, actorId)
  }, [actorId, on, queryClient])
  return null
}

/** Keeps the signed-in person's recent reading on this device while they and their workspace allow it (W1.31). */
export default function OfflineReading({ actorId, workspaceId }: { actorId: string; workspaceId: string | null | undefined }) {
  return workspaceId ? <Keeper actorId={actorId} workspaceId={workspaceId} /> : null
}
