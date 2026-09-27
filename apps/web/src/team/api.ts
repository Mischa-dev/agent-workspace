import { requestJson } from '../api/client'

export type TeamRole = 'owner' | 'admin' | 'member' | 'viewer'

export type Member = {
  id: string
  projectId: string
  actorId: string
  actorKind?: string
  role: TeamRole
  displayName: string
  handle: string
  enabled: boolean
  version: number
  invitedBy: { actorId: string; displayName: string } | null
  acceptedInvitationId: string | null
  acceptedAt: string | null
  disabledByActorId: string | null
  disabledAt: string | null
  createdAt: string
  executionProfileReadiness:
    | 'ready'
    | 'setup_required'
    | 'disabled'
    | 'not_configured'
  capabilities: { changeRole: boolean; disable: boolean; enable: boolean }
}

export type Invitation = {
  id: string
  projectId: string
  role: Exclude<TeamRole, 'owner'>
  tokenPrefix: string
  status: 'active' | 'accepted' | 'revoked' | 'replaced' | 'expired'
  version: number
  issuedByActorId: string
  issuedByDisplayName: string
  expiresAt: string
  acceptedByActorId: string | null
  acceptedAt: string | null
  revokedAt: string | null
  createdAt: string
}

export type IssuedInvitation = Invitation & { invitationUrl: string }

export function listMembers(projectId: string) {
  return requestJson<{ members: Member[] }>(
    `/api/v1/projects/${projectId}/members`,
  )
}

export function updateMember(
  projectId: string,
  actorId: string,
  input: { expectedVersion: number; role?: TeamRole; enabled?: boolean },
) {
  return requestJson<Member>(
    `/api/v1/projects/${projectId}/members/${actorId}`,
    { method: 'PATCH', json: input, idempotencyKey: crypto.randomUUID() },
  )
}

export function listInvitations(projectId: string) {
  return requestJson<Invitation[]>(
    `/api/v1/projects/${projectId}/invitations`,
  )
}

export function issueInvitation(
  projectId: string,
  role: Exclude<TeamRole, 'owner'>,
  expiresInHours: number,
) {
  return requestJson<IssuedInvitation>(
    `/api/v1/projects/${projectId}/invitations`,
    { method: 'POST', json: { role, expiresInHours } },
  )
}

export function revokeInvitation(invitationId: string, expectedVersion: number) {
  return requestJson<Invitation>(
    `/api/v1/project-invitations/${invitationId}/revoke`,
    { method: 'POST', json: { expectedVersion } },
  )
}

export function replaceInvitation(
  invitationId: string,
  expectedVersion: number,
  expiresInHours = 24,
) {
  return requestJson<IssuedInvitation>(
    `/api/v1/project-invitations/${invitationId}/replace`,
    { method: 'POST', json: { expectedVersion, expiresInHours } },
  )
}

export type MemberCandidate = { actorId: string; actorKind: string; displayName: string; handle: string }
export function listMemberCandidates(projectId: string) {
  return requestJson<MemberCandidate[]>(`/api/v1/projects/${projectId}/member-candidates`)
}
export function addExistingMember(projectId: string, actorId: string, role: 'member' | 'viewer') {
  return requestJson<Member>(`/api/v1/projects/${projectId}/members`, { method: 'POST', json: { actorId, role } })
}
