import type { ProjectLocation } from './types'

const supportedKinds = new Set(['project_overview', 'thread', 'settings'])

export function projectLocationHref(
  projectId: string,
  location: ProjectLocation,
): string {
  const project = encodeURIComponent(projectId)
  if (location.kind === 'thread') {
    return `/projects/${project}/threads/${encodeURIComponent(location.conversationId)}`
  }
  if (location.kind === 'settings') return `/projects/${project}/settings`
  return `/projects/${project}`
}

export function safeProjectHref(
  projectId: string,
  href: string | null | undefined,
  location: ProjectLocation,
): string {
  const canonical = projectLocationHref(projectId, location)
  return href === canonical ? href : canonical
}

export function projectLocationFromPath(
  projectId: string,
  pathname: string,
): ProjectLocation | null {
  const projectRoot = `/projects/${encodeURIComponent(projectId)}`
  if (pathname === projectRoot) return { kind: 'project_overview' }
  if (pathname === `${projectRoot}/settings`) return { kind: 'settings' }
  const threadPrefix = `${projectRoot}/threads/`
  if (pathname.startsWith(threadPrefix)) {
    const encodedId = pathname.slice(threadPrefix.length)
    if (!encodedId || encodedId.includes('/')) return null
    try {
      const conversationId = decodeURIComponent(encodedId)
      return conversationId ? { kind: 'thread', conversationId } : null
    } catch {
      return null
    }
  }
  return null
}

export function normalizeLocation(value: unknown): ProjectLocation {
  if (!value || typeof value !== 'object') return { kind: 'project_overview' }
  const candidate = value as { kind?: unknown; conversationId?: unknown }
  if (
    typeof candidate.kind !== 'string' ||
    !supportedKinds.has(candidate.kind)
  ) {
    return { kind: 'project_overview' }
  }
  if (
    candidate.kind === 'thread' &&
    typeof candidate.conversationId === 'string' &&
    candidate.conversationId
  ) {
    return { kind: 'thread', conversationId: candidate.conversationId }
  }
  if (candidate.kind === 'settings') return { kind: 'settings' }
  return { kind: 'project_overview' }
}

export function locationsEqual(
  left: ProjectLocation,
  right: ProjectLocation,
): boolean {
  if (left.kind !== right.kind) return false
  if (left.kind === 'thread' && right.kind === 'thread') {
    return left.conversationId === right.conversationId
  }
  return true
}
