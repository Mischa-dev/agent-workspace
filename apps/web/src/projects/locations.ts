import type { PlainProjectPlace, ProjectLocation } from './types'

/**
 * Where each plain place lives under /projects/:id. Settings aren't a place
 * in the project to come back to: a project that last remembered them (older
 * clients saved it) opens on its overview, so its own tile always leads out of
 * settings.
 */
const PLACE_PATHS: Record<PlainProjectPlace, string> = {
  project_overview: '',
  settings: '',
  work_items: '/work-items',
  milestones: '/milestones',
  timeline: '/timeline',
  planning: '/next',
  pages: '/pages',
  decisions: '/decisions',
  runs: '/launches',
}

/** Paths that open a plain place (the first match wins); a task opens within Tasks. */
const PATH_PLACES: [string, PlainProjectPlace][] = [
  ['/work-items', 'work_items'],
  ['/milestones', 'milestones'],
  ['/timeline', 'timeline'],
  ['/next', 'planning'],
  ['/suggestions', 'planning'],
  ['/graph', 'planning'],
  ['/plan-import', 'planning'],
  ['/decisions', 'decisions'],
  ['/launches', 'runs'],
]

function isPlainPlace(kind: string): kind is PlainProjectPlace {
  return Object.hasOwn(PLACE_PATHS, kind)
}

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
  if (location.kind === 'page') {
    return `/projects/${project}/pages/${encodeURIComponent(location.pageId)}`
  }
  return `/projects/${project}${PLACE_PATHS[location.kind]}`
}

export function safeProjectHref(
  projectId: string,
  href: string | null | undefined,
  location: ProjectLocation,
): string {
  const canonical = projectLocationHref(projectId, location)
  return href === canonical ? href : canonical
}

/** The one id after `prefix` in `pathname`, or null when there isn't exactly one. */
function idAfter(pathname: string, prefix: string) {
  if (!pathname.startsWith(prefix)) return null
  const encodedId = pathname.slice(prefix.length)
  if (!encodedId || encodedId.includes('/')) return null
  try {
    return decodeURIComponent(encodedId) || null
  } catch {
    return null
  }
}

export function projectLocationFromPath(
  projectId: string,
  pathname: string,
): ProjectLocation | null {
  const projectRoot = `/projects/${encodeURIComponent(projectId)}`
  if (pathname === projectRoot) return { kind: 'project_overview' }
  if (pathname === `${projectRoot}/settings`) return { kind: 'settings' }
  if (pathname === `${projectRoot}/pages`) return { kind: 'pages' }
  for (const [path, kind] of PATH_PLACES) {
    if (pathname === `${projectRoot}${path}` || pathname.startsWith(`${projectRoot}${path}/`)) return { kind }
  }
  const pageId = idAfter(pathname, `${projectRoot}/pages/`)
  if (pageId) return { kind: 'page', pageId }
  const channelId = idAfter(pathname, `${projectRoot}/channels/`)
  if (channelId) return { kind: 'channel', conversationId: channelId }
  const threadId = idAfter(pathname, `${projectRoot}/threads/`)
  if (threadId) return { kind: 'thread', conversationId: threadId }
  return null
}

export function normalizeLocation(value: unknown): ProjectLocation {
  if (!value || typeof value !== 'object') return { kind: 'project_overview' }
  const candidate = value as { kind?: unknown; conversationId?: unknown; pageId?: unknown }
  if (typeof candidate.kind !== 'string') return { kind: 'project_overview' }
  if (
    (candidate.kind === 'channel' || candidate.kind === 'thread') &&
    typeof candidate.conversationId === 'string' &&
    candidate.conversationId
  ) {
    return { kind: candidate.kind, conversationId: candidate.conversationId }
  }
  if (candidate.kind === 'page' && typeof candidate.pageId === 'string' && candidate.pageId) {
    return { kind: 'page', pageId: candidate.pageId }
  }
  if (isPlainPlace(candidate.kind)) return { kind: candidate.kind }
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
  if (left.kind === 'page' && right.kind === 'page') return left.pageId === right.pageId
  return true
}
