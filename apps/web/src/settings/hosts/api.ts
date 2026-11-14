import { requestJson } from '../../api/client'

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
  list: (workspaceId: string | null) => ['hosts', workspaceId ?? 'mine'] as const,
}

export const listHosts = (workspaceId: string | null) =>
  requestJson<{ hosts: Host[] }>(
    workspaceId ? `/api/v1/hosts?workspaceId=${encodeURIComponent(workspaceId)}` : '/api/v1/hosts',
  )

export const createEnrolment = (input: { workspaceId?: string; label?: string }) =>
  requestJson<EnrolmentCode>('/api/v1/hosts/enrolments', { method: 'POST', json: input })

export const bindHostAgent = (hostId: string, accountId: string, hostAgentRef: string) =>
  requestJson<Host>(`/api/v1/hosts/${encodeURIComponent(hostId)}/agents`, {
    method: 'POST',
    json: { accountId, hostAgentRef },
  })

export const unbindHostAgent = (hostId: string, accountId: string) =>
  requestJson<Host>(`/api/v1/hosts/${encodeURIComponent(hostId)}/agents/${encodeURIComponent(accountId)}`, {
    method: 'DELETE',
  })

export const unlinkHost = (hostId: string) =>
  requestJson<Host>(`/api/v1/hosts/${encodeURIComponent(hostId)}/unlink`, { method: 'POST', json: {} })
