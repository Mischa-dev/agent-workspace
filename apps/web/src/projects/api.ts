import { requestJson } from '../api/client'
import { isApiError } from '../api/errors'
import { callOp } from '../api/ops'
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
    railPosition: typeof record.railPosition === 'number' && Number.isFinite(record.railPosition) ? record.railPosition : null,
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
    icon: typeof record.icon === 'string' && record.icon ? record.icon : null,
    archivedAt,
    role: asString(record.role),
    permissions: Array.isArray(record.permissions)
      ? record.permissions.filter((entry): entry is string => typeof entry === 'string')
      : [],
    version: Math.max(0, asNumber(record.version)),
    attentionCount: Math.max(0, asNumber(record.attentionCount)),
    modulesOff: record.modulesOff as string[] | undefined,
    kind: record.kind === 'workspace' || record.kind === 'team' || record.kind === 'personal' ? record.kind : 'project',
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

/** Archives a project, against the version the caller read; returns it as it now reads. */
export async function archiveProject(
  projectId: string,
  expectedVersion: number,
): Promise<Project> {
  try {
    await callOp('space_archive', { spaceId: projectId }, { expectedVersion })
  } catch (error) {
    // A retry after a lost answer finds the space already archived, which
    // the server treats as closed; it did what was asked.
    if (!isApiError(error) || error.code !== 'not_found') throw error
    const current = await getProject(projectId).catch(() => null)
    if (!current?.archivedAt) throw error
    return current
  }
  return getProject(projectId)
}

/** Brings an archived project back, against the version the caller read. */
export async function restoreProject(
  projectId: string,
  expectedVersion: number,
): Promise<Project> {
  await callOp('space_restore', { spaceId: projectId }, { expectedVersion })
  return getProject(projectId)
}

/** Renames a project, against the version the caller read. */
export async function renameProject(
  projectId: string,
  name: string,
  expectedVersion: number,
): Promise<Project> {
  await callOp('space_update', { spaceId: projectId, name }, { expectedVersion })
  return getProject(projectId)
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

/** Your pinned projects in your own order, top to bottom; returns the ones placed. */
export async function setRailOrder(projectIds: string[]): Promise<string[]> {
  const payload = asRecord(await requestJson<unknown>('/api/v1/projects/rail-order', { method: 'PUT', json: { projectIds } }))
  return Array.isArray(payload.projectIds) ? payload.projectIds.filter((id): id is string => typeof id === 'string') : []
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
