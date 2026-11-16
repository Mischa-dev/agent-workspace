import { createContext, useCallback, useContext } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import type { OrganizationSummary, SessionResponse } from '../auth/session'
import { CURRENT_WORKSPACE, workspaceHref } from './href'

// The workspace the person is in (W1.14a): the one in the address, else the
// one of the project in the address, else the last one used in this browser,
// else their default. `WorkspaceProvider` (app/WorkspaceBoundary.tsx) works
// it out once for every page; pages and the shell read it here.

const LAST_USED_KEY = 'agent-workspace:workspace'

/** The slug of the workspace last opened in this browser. */
export function lastWorkspaceSlug(): string | null {
  try {
    return window.localStorage.getItem(LAST_USED_KEY)
  } catch {
    return null
  }
}

export function rememberWorkspace(slug: string) {
  try {
    if (window.localStorage.getItem(LAST_USED_KEY) !== slug) window.localStorage.setItem(LAST_USED_KEY, slug)
  } catch {
    // Only a convenience: an address without a workspace falls back to the default.
  }
}

/** The workspace for an address that doesn't name one: the last one used here, else the person's default, else their first. */
export function resolveWorkspace(session: SessionResponse | null | undefined): OrganizationSummary | null {
  if (!session?.user) return null
  const organizations = session.organizations
  const last = lastWorkspaceSlug()
  return (
    organizations.find((organization) => organization.slug === last) ??
    organizations.find((organization) => organization.id === session.user?.defaultOrganizationId) ??
    organizations[0] ??
    null
  )
}

/** Undefined outside the provider, so a page rendered on its own (a test) still finds a workspace. */
export const WorkspaceContext = createContext<OrganizationSummary | null | undefined>(undefined)

/** The workspace the person is in. */
export function useWorkspace(): OrganizationSummary | null {
  const provided = useContext(WorkspaceContext)
  const session = useQueryClient().getQueryData<SessionResponse>(queryKeys.session)
  return provided !== undefined ? provided : resolveWorkspace(session)
}

/** The id of the workspace the person is in: what Home, its sidebar and the palette follow. */
export function useWorkspaceId(): string | null {
  return useWorkspace()?.id ?? null
}

/** Builds addresses in the workspace the person is in: `href('/inbox')` is `/w/<slug>/inbox`. */
export function useWorkspaceHref(): (path?: string) => string {
  const slug = useWorkspace()?.slug ?? CURRENT_WORKSPACE
  return useCallback((path?: string) => workspaceHref(slug, path), [slug])
}
