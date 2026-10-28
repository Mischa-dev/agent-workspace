import { requestJson } from '../api/client'
import { normalizeLocation, safeProjectHref } from './locations'
import type {
  Project,
  ProjectCapabilities,
  ProjectListResponse,
  ProjectNavigation,
  ProjectSwitchResponse,
} from './types'

type WireRecord = Record<string, unknown>

function asRecord(value: unknown): WireRecord {
  return value && typeof value === 'object' ? (value as WireRecord) : {}
}

function asString(value: unknown, fallback = '') {
  return typeof value === 'string' ? value : fallback
}

function asNullableString(value: unknown) {
  return typeof value === 'string' ? value : null
}

function asNumber(value: unknown, fallback = 0) {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

function asBoolean(value: unknown, fallback = false) {
  return typeof value === 'boolean' ? value : fallback
}

function normalizeCapabilities(value: unknown): ProjectCapabilities {
  const record = asRecord(value)
  return {
    view: asBoolean(record.view),
    manage: asBoolean(record.manage),
    archive: asBoolean(record.archive),
    navigationWrite: asBoolean(record.navigationWrite),
  }
}

export function normalizeNavigation(
  projectId: string,
  value: unknown,
): ProjectNavigation {
  const record = asRecord(value)
  const location = normalizeLocation(record.location)
  const pinnedAt = asNullableString(record.pinnedAt)
  return {
    location,
    href: safeProjectHref(
      projectId,
      asNullableString(record.href),
      location,
    ),
    pinned: asBoolean(record.pinned, Boolean(pinnedAt)),
    lastOpenedAt: asNullableString(record.lastOpenedAt),
    version: Math.max(0, asNumber(record.version)),
    contextDrawerOpen:
      typeof record.contextDrawerOpen === 'boolean'
        ? record.contextDrawerOpen
        : undefined,
    filters:
      record.filters && typeof record.filters === 'object'
        ? (record.filters as Record<string, unknown>)
        : undefined,
  }
}

export function normalizeProject(value: unknown): Project {
  const record = asRecord(value)
  const id = asString(record.id)
  const archivedAt = asNullableString(record.archivedAt)
  const navigation = normalizeNavigation(id, record.navigation)
  const overviewLocation = { kind: 'project_overview' } as const
  return {
    id,
    organizationId: asString(record.organizationId),
    key: asString(record.key, asString(record.projectKey)),
    name: asString(record.name),
    archivedAt,
    role: asString(record.role),
    version: Math.max(0, asNumber(record.version)),
    attentionCount: Math.max(0, asNumber(record.attentionCount)),
    capabilities: normalizeCapabilities(record.capabilities),
    navigation: archivedAt
      ? {
          ...navigation,
          location: overviewLocation,
          href: safeProjectHref(id, null, overviewLocation),
        }
      : navigation,
  }
}

export async function listProjects(query?: string, options: { archived?: boolean } = {}): Promise<ProjectListResponse> {
  const parameters = new URLSearchParams()
  if (query?.trim()) parameters.set('q', query.trim())
  parameters.set('limit', '100')
  if (options.archived) parameters.set('archived', 'true')
  const suffix = parameters.toString()
  const payload = await requestJson<{ projects?: unknown[] } | unknown[]>(
    `/api/v1/projects?${suffix}`,
  )
  const projects = Array.isArray(payload)
    ? payload
    : Array.isArray(payload.projects)
      ? payload.projects
      : []
  return { projects: projects.map(normalizeProject) }
}

export async function createProject(input: {
  organizationId: string
  key: string
  name: string
  templateId?: string
  idempotencyKey: string
}): Promise<Project> {
  const payload = await requestJson<unknown>('/api/v1/projects', {
    method: 'POST',
    idempotencyKey: input.idempotencyKey,
    json: {
      organizationId: input.organizationId,
      key: input.key,
      name: input.name,
      ...(input.templateId ? { templateId: input.templateId } : {}),
    },
  })
  const record = asRecord(payload)
  return normalizeProject(record.project ?? payload)
}

export async function getProject(projectId: string): Promise<Project> {
  const payload = await requestJson<unknown>(
    `/api/v1/projects/${encodeURIComponent(projectId)}`,
  )
  const record = asRecord(payload)
  return normalizeProject(record.project ?? payload)
}

export async function archiveProject(
  projectId: string,
  expectedVersion: number,
): Promise<Project> {
  const payload = await requestJson<unknown>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/archive`,
    {
      method: 'POST',
      json: { expectedVersion },
    },
  )
  const record = asRecord(payload)
  return normalizeProject(record.project ?? payload)
}

/** Brings an archived project back, against the version the caller read. */
export async function restoreProject(
  projectId: string,
  expectedVersion: number,
): Promise<Project> {
  const payload = await requestJson<unknown>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/restore`,
    {
      method: 'POST',
      json: { expectedVersion },
    },
  )
  const record = asRecord(payload)
  return normalizeProject(record.project ?? payload)
}

export async function getProjectNavigation(
  projectId: string,
): Promise<ProjectNavigation> {
  const payload = await requestJson<unknown>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/navigation`,
  )
  const record = asRecord(payload)
  return normalizeNavigation(projectId, record.navigation ?? payload)
}

export async function updateProjectNavigation(
  projectId: string,
  input: {
    location: ProjectNavigation['location']
    expectedVersion: number
    pinned?: boolean
    contextDrawerOpen?: boolean
    filters?: Record<string, unknown>
  },
): Promise<ProjectNavigation> {
  const payload = await requestJson<unknown>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/navigation`,
    {
      method: 'PUT',
      json: input,
    },
  )
  const record = asRecord(payload)
  return normalizeNavigation(projectId, record.navigation ?? payload)
}

export async function switchProject(
  projectId: string,
  expectedVersion: number,
): Promise<ProjectSwitchResponse> {
  const payload = asRecord(
    await requestJson<unknown>(
      `/api/v1/projects/${encodeURIComponent(projectId)}/switch`,
      { method: 'POST', json: { expectedVersion } },
    ),
  )
  const project = normalizeProject(payload.project)
  const navigation = normalizeNavigation(projectId, payload.navigation)
  return {
    project: { ...project, navigation },
    navigation,
    href: safeProjectHref(projectId, asNullableString(payload.href), navigation.location),
  }
}
