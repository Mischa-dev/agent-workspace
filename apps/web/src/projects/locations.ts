import type { ProjectLocation } from './types'

const supportedKinds = new Set([
  'project_overview',
  'channel',
  'thread',
  'work_items',
  'settings',
])

export function projectLocationHref(
  projectId: string,
  location: ProjectLocation,
): string {
  const project = encodeURIComponent(projectId)
  if (location.kind === 'channel') {
    return `/projects/${project}/channels/${encodeURIComponent(location.conversationId)}`
  }
  if (location.kind === 'thread') {
    return `/projects/${project}/threads/${encodeURIComponent(location.conversationId)}`
  }
  // Settings aren't a place in the Project to come back to: opening a Project
  // that last remembered them (older clients saved it) lands on its overview,
  // so the Project's own tile always leads out of settings.
  if (location.kind === 'work_items') return `/projects/${project}/work-items`
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
  if (
    pathname === `${projectRoot}/work-items` ||
    pathname.startsWith(`${projectRoot}/work-items/`)
  ) {
    return { kind: 'work_items' }
  }
  const channelPrefix = `${projectRoot}/channels/`
  if (pathname.startsWith(channelPrefix)) {
    const encodedId = pathname.slice(channelPrefix.length)
    if (!encodedId || encodedId.includes('/')) return null
    try {
      const conversationId = decodeURIComponent(encodedId)
      return conversationId ? { kind: 'channel', conversationId } : null
    } catch {
      return null
    }
  }
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
    (candidate.kind === 'channel' || candidate.kind === 'thread') &&
    typeof candidate.conversationId === 'string' &&
    candidate.conversationId
  ) {
    return { kind: candidate.kind, conversationId: candidate.conversationId }
  }
  if (candidate.kind === 'settings') return { kind: 'settings' }
  if (candidate.kind === 'work_items') return { kind: 'work_items' }
  return { kind: 'project_overview' }
}

export function locationsEqual(
  left: ProjectLocation,
  right: ProjectLocation,
): boolean {
  if (left.kind !== right.kind) return false
  if (
    (left.kind === 'channel' || left.kind === 'thread') &&
    (right.kind === 'channel' || right.kind === 'thread')
  ) {
    return left.conversationId === right.conversationId
  }
  return true
}
