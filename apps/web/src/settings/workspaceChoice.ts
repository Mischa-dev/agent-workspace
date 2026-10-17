import { useMutation, useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import type { AuthenticatedSession } from '../auth/session'
import { toastError } from '../app/mutations'
import { setDefaultWorkspace } from './account/api'

/** The workspace settings are about: the one the person chose, else their first. */
export function defaultWorkspace(session: AuthenticatedSession) {
  return (
    session.organizations.find((organization) => organization.id === session.user.defaultOrganizationId) ??
    session.organizations[0] ??
    null
  )
}

/** Make another workspace the default (it also decides which one Messages uses), then reread the session. */
export function useSwitchWorkspace() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: setDefaultWorkspace,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.session }),
    onError: (error) => {
      toastError('Couldn’t switch workspace', error)
    },
  })
}
