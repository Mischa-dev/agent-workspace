import { requestJson } from '../api/client'
import { callOp } from '../api/ops'
import type { SpaceMemberAddOutput } from '../api/generated/operations'

export type TeamRole = 'owner' | 'admin' | 'member' | 'viewer'

export type Member = {
  id: string
  projectId: string
  actorId: string
  actorKind?: string
  role: TeamRole
  /** The workspace's custom role they hold here (AWD-74). */
  roleId?: string | null
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
  /** The roles you could give them (a built-in role's key or a custom role's ID), from the server. */
  assignableRoles?: string[]
}

export type Invitation = {
  id: string
  projectId: string
  role: Exclude<TeamRole, 'owner'>
  roleId?: string | null
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

/** A new join link, shown once. */
export type IssuedInvitation = { invitationUrl: string }

export function listMembers(projectId: string) {
  return requestJson<{ members: Member[] }>(
    `/api/v1/projects/${projectId}/members`,
  )
}

/** What a change to someone's membership returns: enough to name them and undo it. */
export type MemberChange = { actorId: string; displayName: string; version: number }

function changed(member: SpaceMemberAddOutput): MemberChange {
  return { actorId: member.accountId, displayName: member.name, version: member.version }
}

/** Gives someone another role, or turns their access off or back on. */
export async function updateMember(
  projectId: string,
  actorId: string,
  input: { expectedVersion: number; role?: TeamRole; roleId?: string; enabled?: boolean },
): Promise<MemberChange> {
  const who = { spaceId: projectId, account: actorId }
  if (input.enabled === false) return changed(await callOp('space_member_remove', { ...who, expectedVersion: input.expectedVersion }))
  if (input.enabled === true) return changed(await callOp('space_member_add', { ...who, expectedVersion: input.expectedVersion }))
  const role = input.roleId ?? input.role ?? 'member'
  return changed(await callOp('space_member_update', { ...who, role, expectedVersion: input.expectedVersion }))
}

export function listInvitations(projectId: string) {
  return requestJson<Invitation[]>(
    `/api/v1/projects/${projectId}/invitations`,
  )
}

/**
 * An invitation's link is shown once: a retried request the server had
 * already done answers without it. Say so rather than offer an empty link.
 */
function joinUrl(url: string | null | undefined): string {
  if (!url) throw new Error("Its link didn't arrive. Replace the invitation from the list to get a new one.")
  return url
}

export async function issueInvitation(
  projectId: string,
  role: Exclude<TeamRole, 'owner'>,
  expiresInHours: number,
  roleId?: string,
): Promise<IssuedInvitation> {
  const issued = await callOp('invitation_create', { projectId, role: roleId ?? role, expiresInHours })
  return { invitationUrl: joinUrl(issued.joinUrl) }
}

export async function revokeInvitation(projectId: string, invitationId: string, expectedVersion: number) {
  await callOp('invitation_update', { projectId, invitation: invitationId, revoke: true, expectedVersion })
}

export async function replaceInvitation(
  projectId: string,
  invitationId: string,
  expectedVersion: number,
  expiresInHours = 24,
): Promise<IssuedInvitation> {
  const issued = await callOp('invitation_update', { projectId, invitation: invitationId, replace: true, expectedVersion, expiresInHours })
  return { invitationUrl: joinUrl(issued.joinUrl) }
}

export type MemberCandidate = { actorId: string; actorKind: string; displayName: string; handle: string }
export const teamKeys = {
  memberCandidates: (projectId: string) => ['member-candidates', projectId] as const,
}

export function listMemberCandidates(projectId: string) {
  return requestJson<MemberCandidate[]>(`/api/v1/projects/${projectId}/member-candidates`)
}
export async function addExistingMember(projectId: string, actorId: string, role: 'member' | 'viewer', roleId?: string): Promise<MemberChange> {
  return changed(await callOp('space_member_add', { spaceId: projectId, account: actorId, role: roleId ?? role }))
}
