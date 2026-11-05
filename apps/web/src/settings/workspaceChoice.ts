import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import { queryKeys } from '../api/queryKeys'
import type { AuthenticatedSession } from '../auth/session'
import { toastError } from '../app/mutations'
import { setDefaultWorkspace } from './account/api'
import { notificationKeys } from './account/usageApi'

/** The workspace settings are about: the one the person chose, else their first. */
export function defaultWorkspace(session: AuthenticatedSession) {
  return (
    session.organizations.find((organization) => organization.id === session.user.defaultOrganizationId) ??
    session.organizations[0] ??
    null
  )
}

const VIEWED_WORKSPACE_KEY = 'agent-workspace:settings:workspace'

function rememberedWorkspace() {
  try {
    return window.sessionStorage.getItem(VIEWED_WORKSPACE_KEY)
  } catch {
    return null
  }
}

/**
 * The workspace whose settings are open: the one last picked in settings this
 * visit, else the default. Picking one only changes what settings show;
 * making it the default is its own step.
 */
export function useViewedWorkspace(session: AuthenticatedSession) {
  const [picked, setPicked] = useState(rememberedWorkspace)
  const workspace = session.organizations.find((organization) => organization.id === picked) ?? defaultWorkspace(session)
  const view = (organizationId: string) => {
    setPicked(organizationId)
    try {
      window.sessionStorage.setItem(VIEWED_WORKSPACE_KEY, organizationId)
    } catch {
      // Only a convenience: settings fall back to the default workspace.
    }
  }
  return [workspace, view] as const
}

/**
 * Make another workspace the default (it also decides which one Messages and
 * notification settings use), then reread the session and those settings.
 */
export function useSwitchWorkspace() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: setDefaultWorkspace,
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.session }),
        queryClient.invalidateQueries({ queryKey: notificationKeys.settings }),
      ]),
    onError: (error) => {
      toastError('Couldn’t switch workspace', error)
    },
  })
}
