import { requestJson } from '../api/client'
import {
  normalizeSessionResponse,
  type SessionResponse,
} from '../auth/session'

/**
 * Workspace invitations start `aw_wsi_`; Project invitations start `aw_inv_`;
 * invitation links, which several people may use, start `aw_lnk_`.
 */
export type InvitationKind = 'workspace' | 'project' | 'link'

export type ResolvedInvitation = {
  kind: InvitationKind
  organizationName: string
  /** Null for a workspace invitation, and for a link that names no space. */
  projectName: string | null
  role: 'admin' | 'member' | 'viewer'
  expiresAt: string
}

export function invitationKind(token: string): InvitationKind {
  if (token.startsWith('aw_wsi_')) return 'workspace'
  if (token.startsWith('aw_lnk_')) return 'link'
  return 'project'
}

const INVITATION_PATHS: Record<InvitationKind, string> = {
  workspace: '/api/v1/workspace-invitations',
  project: '/api/v1/project-invitations',
  link: '/api/v1/invite-links',
}

function invitationPath(token: string, action: 'resolve' | 'accept-new' | 'accept-existing') {
  return `${INVITATION_PATHS[invitationKind(token)]}/${action}`
}

function normalizeResolvedInvitation(value: unknown, kind: InvitationKind): ResolvedInvitation {
  if (!value || typeof value !== 'object') {
    throw new Error('Invitation response was malformed.')
  }
  const record = value as Record<string, unknown>
  const projectName = typeof record.projectName === 'string' ? record.projectName : null
  if (
    typeof record.organizationName !== 'string' ||
    (kind === 'project' && projectName === null) ||
    (record.role !== 'admin' &&
      record.role !== 'member' &&
      record.role !== 'viewer') ||
    typeof record.expiresAt !== 'string'
  ) {
    throw new Error('Invitation response was malformed.')
  }
  return {
    kind,
    organizationName: record.organizationName,
    projectName: kind === 'workspace' ? null : projectName,
    role: record.role,
    expiresAt: record.expiresAt,
  }
}

export async function resolveInvitation(token: string) {
  return normalizeResolvedInvitation(
    await requestJson<unknown>(invitationPath(token, 'resolve'), {
      method: 'POST',
      json: { token },
    }),
    invitationKind(token),
  )
}

export async function acceptNewInvitation(input: {
  token: string
  loginName: string
  displayName: string
  handle: string
  password: string
}): Promise<SessionResponse> {
  return normalizeSessionResponse(
    await requestJson<unknown>(invitationPath(input.token, 'accept-new'), {
      method: 'POST',
      json: input,
    }),
  )
}

export async function acceptExistingInvitation(input: {
  token: string
  handle: string
}): Promise<SessionResponse> {
  return normalizeSessionResponse(
    await requestJson<unknown>(invitationPath(input.token, 'accept-existing'), {
      method: 'POST',
      json: input,
    }),
  )
}
