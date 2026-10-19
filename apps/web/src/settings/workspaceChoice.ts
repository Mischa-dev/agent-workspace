import { useMutation, useQueryClient } from '@tanstack/react-query'

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
