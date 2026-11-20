import { useEffect } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { useOfflineGate, type WorkspaceRef } from './offlineGate'
import { offlineOwner } from './offlinePointer'
import { clearOffline, keepOffline } from './persist'

function Keeper({ actorId, workspaceId, workspaces }: { actorId: string; workspaceId: string; workspaces: readonly WorkspaceRef[] }) {
  const queryClient = useQueryClient()
  // On in this workspace, and allowed in every workspace the person is in:
  // the kept copy holds what they read in all of them. Unknown (offline, or
  // still loading) changes nothing.
  const { on } = useOfflineGate(workspaceId, workspaces)
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
export default function OfflineReading({
  actorId,
  workspaceId,
  workspaces,
}: {
  actorId: string
  workspaceId: string | null | undefined
  workspaces: readonly WorkspaceRef[]
}) {
  return workspaceId ? <Keeper actorId={actorId} workspaceId={workspaceId} workspaces={workspaces} /> : null
}
