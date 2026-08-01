import { requestJson } from '../api/client'
import {
  normalizeSessionResponse,
  type SessionResponse,
} from '../auth/session'

export type ResolvedInvitation = {
  organizationName: string
  projectName: string
  role: 'admin' | 'member' | 'viewer'
  expiresAt: string
}

function normalizeResolvedInvitation(value: unknown): ResolvedInvitation {
  if (!value || typeof value !== 'object') {
    throw new Error('Invitation response was malformed.')
  }
  const record = value as Record<string, unknown>
  if (
    typeof record.organizationName !== 'string' ||
    typeof record.projectName !== 'string' ||
    (record.role !== 'admin' &&
      record.role !== 'member' &&
      record.role !== 'viewer') ||
    typeof record.expiresAt !== 'string'
  ) {
    throw new Error('Invitation response was malformed.')
  }
  return {
    organizationName: record.organizationName,
    projectName: record.projectName,
    role: record.role,
    expiresAt: record.expiresAt,
  }
}

export async function resolveInvitation(token: string) {
  return normalizeResolvedInvitation(
    await requestJson<unknown>('/api/v1/project-invitations/resolve', {
      method: 'POST',
      json: { token },
    }),
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
    await requestJson<unknown>('/api/v1/project-invitations/accept-new', {
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
    await requestJson<unknown>('/api/v1/project-invitations/accept-existing', {
      method: 'POST',
      json: input,
    }),
  )
}
