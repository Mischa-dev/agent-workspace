import {
  createContext,
  createElement,
  useContext,
  type ReactNode,
} from 'react'
import {
  useQuery,
  useQueryClient,
  type QueryClient,
} from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { clearLocalDrafts, setLocalDraftOwner } from '../app/localDrafts'
import { things } from '../data/things'
import { forgetOffline, offlineOwner } from '../data/offlinePointer'
import { queryKeys } from '../api/queryKeys'
import { resetQueryScope } from '../api/queryScope'
import { closeCommandMenu } from '../shell/commandStore'
import { clearToasts } from '../ui/toastStore'
import { clearPendingMoves } from '../work-items/moveStore'

export type OrganizationRole = 'owner' | 'admin' | 'member'

export type OrganizationSummary = {
  id: string
  name: string
  slug: string
  role: OrganizationRole
  /** What the caller may do in this workspace (K1.26); ask with `can`, never the role's name. */
  permissions: string[]
  /** `suspended` or `deleting` when the workspace refuses changes (H1.24); missing means active. */
  lifecycle?: 'active' | 'importing' | 'suspended' | 'deleting'
}

export type SessionUser = {
  id: string
  actorId: string
  actorKind: string
  loginName: string
  displayName: string
  /** The workspace the person last chose; null means "the first one". */
  defaultOrganizationId?: string | null
  capabilities: {
    createProject: boolean
    /** Runs this server, so sees Settings › Workspace › Server. */
    serverStatus?: boolean
  }
}

export type SessionResponse = {
  authenticated: boolean
  user: SessionUser | null
  organizations: OrganizationSummary[]
}

export type AuthenticatedSession = SessionResponse & {
  authenticated: true
  user: SessionUser
}

type WireRecord = Record<string, unknown>

const signedOutSession: SessionResponse = {
  authenticated: false,
  user: null,
  organizations: [],
}

const uuidPattern =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
const loginNamePattern = /^[a-z0-9][a-z0-9._-]{1,62}[a-z0-9]$/
const organizationSlugPattern = /^[a-z0-9]+(?:-[a-z0-9]+)*$/

const AuthenticatedSessionContext =
  createContext<AuthenticatedSession | null>(null)

function asRecord(value: unknown): WireRecord {
  return value && typeof value === 'object' ? (value as WireRecord) : {}
}

function boundedText(value: unknown, maximumLength: number): string | null {
  if (typeof value !== 'string') return null
  const normalized = value.trim()
  if (
    !normalized ||
    normalized.length > maximumLength ||
    Array.from(normalized).some((character) => {
      const codePoint = character.codePointAt(0) ?? 0
      return codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f)
    })
  ) {
    return null
  }
  return normalized
}

function normalizedUuid(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const normalized = value.toLowerCase()
  return uuidPattern.test(normalized) ? normalized : null
}

function isOrganizationRole(value: unknown): value is OrganizationRole {
  return value === 'owner' || value === 'admin' || value === 'member'
}

function normalizeOrganization(value: unknown): OrganizationSummary | null {
  const record = asRecord(value)
  const id = normalizedUuid(record.id)
  const name = boundedText(record.name, 120)
  const slug =
    typeof record.slug === 'string' &&
    record.slug.length <= 64 &&
    organizationSlugPattern.test(record.slug)
      ? record.slug
      : null
  const role = isOrganizationRole(record.role) ? record.role : null
  const lifecycle =
    record.lifecycle === 'importing' || record.lifecycle === 'suspended' || record.lifecycle === 'deleting'
      ? record.lifecycle
      : undefined
  if (!id || !name || !slug || !role) return null
  const permissions = Array.isArray(record.permissions)
    ? record.permissions.filter((entry): entry is string => typeof entry === 'string')
    : []
  return lifecycle ? { id, name, slug, role, permissions, lifecycle } : { id, name, slug, role, permissions }
}

function normalizeUser(value: unknown): SessionUser | null {
  const record = asRecord(value)
  const id = normalizedUuid(record.id)
  const actorId = normalizedUuid(record.actorId)
  const loginName =
    typeof record.loginName === 'string' &&
    loginNamePattern.test(record.loginName)
      ? record.loginName
      : null
  const displayName = boundedText(record.displayName, 120)
  if (
    !id ||
    !actorId ||
    record.actorKind !== 'human' ||
    !loginName ||
    !displayName
  ) {
    return null
  }
  return {
    id,
    actorId,
    actorKind: 'human',
    loginName,
    displayName,
    defaultOrganizationId: normalizedUuid(record.defaultOrganizationId),
    capabilities: {
      createProject: asRecord(record.capabilities).createProject === true,
      serverStatus: asRecord(record.capabilities).serverStatus === true,
    },
  }
}

export function normalizeSessionResponse(value: unknown): SessionResponse {
  const record = asRecord(value)
  if (record.authenticated !== true) return signedOutSession
  const user = normalizeUser(record.user)
  if (!user) return signedOutSession
  const organizations = Array.isArray(record.organizations)
    ? record.organizations
        .map(normalizeOrganization)
        .filter(
          (organization): organization is OrganizationSummary =>
            organization !== null,
        )
    : []
  return {
    authenticated: true,
    user,
    organizations,
  }
}

export async function getSession() {
  const session = normalizeSessionResponse(
    await requestJson<unknown>('/api/v1/auth/session'),
  )
  // Drafts are kept per person; read only this one's.
  setLocalDraftOwner(session.user?.actorId ?? null)
  return session
}

export async function logout() {
  // Drafts and unsent messages stay on this device only while signed in;
  // forget them first, so a failed or slow sign-out can't leave them behind.
  clearLocalDrafts()
  setLocalDraftOwner(null)
  // And the writes waiting for the connection (W1.30), kept in IndexedDB.
  await import('../data/outbound').then((outbound) => outbound.clearOutbound()).catch(() => undefined)
  // And what was kept for reading offline (W1.31).
  forgetOffline()
  await requestJson<void>('/api/v1/auth/logout', { method: 'POST' })
}

export function useSession() {
  const queryClient = useQueryClient()
  return useQuery({
    queryKey: queryKeys.session,
    queryFn: () => confirmSession(queryClient),
    staleTime: 30_000,
    retry: false,
  })
}

/**
 * Asks the server who is signed in. A session shown before the answer (one
 * kept for reading offline, or a tab left open) may be someone else's: when
 * the answer names another account, what the earlier one read is cleared,
 * the copy kept on this device included, before the new one renders.
 */
export async function confirmSession(queryClient: QueryClient) {
  const shown = queryClient.getQueryData<SessionResponse>(queryKeys.session)
  const session = await getSession()
  const before = isAuthenticatedSession(shown) ? shown.user.actorId : null
  const now = isAuthenticatedSession(session) ? session.user.actorId : null
  if (before && now && before !== now) {
    if (offlineOwner()) forgetOffline()
    resetQueryScope()
    clearActorScopedQueries(queryClient)
  }
  return session
}

export function clearAuthenticatedQueryCache(queryClient: QueryClient) {
  // Signed out (or the server says the session is gone): nothing stays for reading offline (W1.31).
  if (offlineOwner()) forgetOffline()
  resetQueryScope()
  clearActorScopedQueries(queryClient)
  setLocalDraftOwner(null)
  queryClient.setQueryData<SessionResponse>(queryKeys.session, signedOutSession)
}

export function installAuthenticatedSession(
  queryClient: QueryClient,
  session: AuthenticatedSession,
) {
  resetQueryScope()
  clearActorScopedQueries(queryClient)
  setLocalDraftOwner(session.user.actorId)
  queryClient.setQueryData<SessionResponse>(queryKeys.session, session)
}

function clearActorScopedQueries(queryClient: QueryClient) {
  queryClient.removeQueries({
    predicate: (query) =>
      !(
        query.queryKey.length === queryKeys.session.length &&
        query.queryKey.every(
          (segment, index) => segment === queryKeys.session[index],
        )
      ),
  })
  queryClient.getMutationCache().clear()
  // Toasts can name private conversations and hold Undo callbacks.
  clearToasts()
  // What the last account read is theirs (W1.5).
  things.clear()
  // A move whose answer never arrived belongs to whoever sent it (AWD-102).
  clearPendingMoves()
  // A menu left open must not reopen over the next session.
  closeCommandMenu()
}

export function isAuthenticatedSession(
  session: SessionResponse | undefined,
): session is AuthenticatedSession {
  return session?.authenticated === true && session.user !== null
}

export function AuthenticatedSessionProvider({
  children,
  session,
}: {
  children: ReactNode
  session: AuthenticatedSession
}) {
  return createElement(
    AuthenticatedSessionContext.Provider,
    { value: session },
    children,
  )
}

export function useAuthenticatedSession() {
  const session = useContext(AuthenticatedSessionContext)
  if (!session) {
    throw new Error('Authenticated route rendered without a session.')
  }
  return session
}
