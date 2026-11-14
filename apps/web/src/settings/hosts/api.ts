import { callOp } from '../../api/ops'

/** A1.11: hosts, the machines and services that run agents. */
export type HostOwner = { kind: 'person' | 'workspace'; id: string; name: string }

export type HostAgent = {
  accountId: string
  name: string | null
  hostAgentRef: string
  status: 'active' | 'paused' | 'gone'
}

export type DescribedAgent = { hostAgentRef: string; name: string | null }

export type Host = {
  id: string
  name: string
  owner: HostOwner
  builtin: boolean
  status: 'active' | 'offline' | 'revoked'
  software: string | null
  softwareVersion: string | null
  protocol: string
  capabilities: Record<string, unknown>
  offers: { id: string; title: string; summary?: string }[]
  actions: { id: string; label: string }[]
  describedAgents: DescribedAgent[]
  agents: HostAgent[]
  lastSeenAt: string | null
  canManage: boolean
  version: number
  createdAt: string
}

export type EnrolmentCode = { enrolmentId: string; code: string; expiresAt: string; owner: HostOwner }

export const hostKeys = {
  list: (workspaceId: string | null, mineOnly: boolean) => ['hosts', workspaceId ?? 'only', mineOnly ? 'mine' : 'all'] as const,
}

// The host operations (hosts/ops.rs). `workspaceId` names where the call
// acts; left out, the caller's only workspace.
const where = (workspaceId: string | null) => (workspaceId ? { workspaceId } : {})

export const listHosts = (workspaceId: string | null, mineOnly: boolean) =>
  callOp('host_list', mineOnly ? { mineOnly } : {}, where(workspaceId)).then((value) => ({
    hosts: value.hosts as unknown as Host[],
  }))

export const createEnrolment = (workspaceId: string | null, input: { owner: 'person' | 'workspace'; label?: string }) =>
  callOp('host_enrolment_create', input, where(workspaceId)).then((value) => value as unknown as EnrolmentCode)

export const bindHostAgent = (workspaceId: string | null, hostId: string, accountId: string, hostAgentRef: string) =>
  callOp('host_agent_bind', { hostId, accountId, hostAgentRef }, where(workspaceId)).then(
    (value) => value as unknown as Host,
  )

export const unbindHostAgent = (workspaceId: string | null, hostId: string, accountId: string) =>
  callOp('host_agent_unbind', { hostId, accountId }, where(workspaceId)).then((value) => value as unknown as Host)

export const unlinkHost = (workspaceId: string | null, hostId: string) =>
  callOp('host_unlink', { hostId }, where(workspaceId)).then((value) => value as unknown as Host)
