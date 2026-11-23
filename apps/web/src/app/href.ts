// Addresses (W1.14a, W1.14b): a workspace's own pages live under
// `/w/<slug>/...`, a space's under `/w/<slug>/s/<spaceId>/...`, any thing at
// `/w/<slug>/t/<kind>/<id>`, and the workspace's settings under
// `/w/<slug>/settings/...`. Links are built here, never pasted.
//
// The grammar is a contract with the server's `addresses` module
// (apps/server/src/addresses.rs): `workspacePath` here and `workspace_path`
// there turn an old address into a new one the same way, and a change to a
// word changes both.

/**
 * Stands for "the workspace you used last" in an address, such as the app's
 * home-screen shortcuts, which can't know a slug. `-` is never a slug.
 */
export const CURRENT_WORKSPACE = '-'

/** Pages whose word in the workspace's address differs from their old address. */
const RENAMED: readonly (readonly [legacy: string, now: string])[] = [
  ['/projects', '/spaces'],
  ['/new-task', '/new'],
]

/** First segments that stay at the root: account settings, the kit, the signed-out pages and the workspace form itself. */
const ROOT_ONLY = new Set(['settings', 'design-system', 'w', 'forms', 'sign-in', 'forgot-password', 'reset-password', 'verify-email', 'join', 'setup'])

/** A task key such as `AWD-12`, which opens its task wherever it is. */
const TASK_KEY = /^[A-Za-z][A-Za-z0-9]{1,9}-[1-9][0-9]{0,8}$/

const split = (path: string) => {
  const at = path.search(/[?#]/)
  return at === -1 ? [path, ''] : [path.slice(0, at), path.slice(at)]
}

const segmentsOf = (pathname: string) => pathname.split('/').filter(Boolean)
const joined = (segments: readonly string[]) => segments.map((segment) => `/${segment}`).join('')

/**
 * The workspace whose addresses links are built in: the one the person is
 * in, set by `WorkspaceProvider` as it renders (before any page below it),
 * else `-` (the last one used), which `/w/-/...` resolves.
 */
let currentSlug: string = CURRENT_WORKSPACE

/** Called by the workspace provider: links built after this go to `slug`. */
export function setCurrentWorkspaceSlug(slug: string | null | undefined) {
  currentSlug = slug || CURRENT_WORKSPACE
}

/** The slug links are built in now. */
export function currentWorkspaceSlug(): string {
  return currentSlug
}

/**
 * A place inside a space, from today's words to the address's: `work-items`
 * is `tasks`, `launches` is `runs`, `channels/<id>` is `chat/<id>`, the
 * planning pages sit under `planning`. Mirrors the server's `space_tail`.
 */
function spaceTail(tail: readonly string[]): string[] {
  const [first, ...rest] = tail
  switch (first) {
    case undefined:
      return []
    case 'work-items':
      return ['tasks', ...rest]
    case 'launches':
      return ['runs', ...rest]
    case 'channels':
    case 'conversations':
      return ['chat', ...rest]
    case 'next':
    case 'suggestions':
    case 'graph':
      return ['planning', ...tail]
    case 'plan-import':
      return ['planning', 'import', ...rest]
    default:
      return [...tail]
  }
}

/** The reverse of `spaceTail`: an address's words back to today's. */
function legacySpaceTail(tail: readonly string[]): string[] {
  const [first, second, ...rest] = tail
  switch (first) {
    case undefined:
      return []
    case 'tasks':
      return ['work-items', ...tail.slice(1)]
    case 'runs':
      return ['launches', ...tail.slice(1)]
    case 'chat':
      if (second === 'archived' || second === 'channels') return ['conversations', second, ...rest]
      return ['channels', ...tail.slice(1)]
    case 'planning':
      if (second === 'import') return ['plan-import', ...rest]
      return second ? [second, ...rest] : ['next']
    default:
      return [...tail]
  }
}

/**
 * An old address's path inside `/w/<slug>`, or null when it isn't a
 * workspace's page. Takes route patterns too (`/projects/:projectId/work-items`
 * is `/s/:projectId/tasks`). Mirrors the server's `workspace_path`.
 */
function workspaceInner(pathname: string): string | null {
  const segments = segmentsOf(pathname)
  const [first, second, third, ...rest] = segments
  if (!first) return '/home'
  if (first === 'projects') {
    if (second === undefined) return '/spaces'
    if (second === 'new' && third === undefined) return '/spaces/new'
    if (third === 'settings') return `/settings/spaces/${second}${joined(rest)}`
    return `/s/${second}${joined(spaceTail(segments.slice(2)))}`
  }
  if (first === 'settings') {
    if (second === 'workspace') return `/settings${joined(segments.slice(2))}`
    if (second === 'projects') return `/settings/spaces${joined(segments.slice(2))}`
    return null
  }
  // A DM's own address; the route pattern `/messages/:conversationId` stays a route of its own.
  if (first === 'messages' && second && second !== 'agents' && !second.startsWith(':') && third === undefined) return `/dm/${second}`
  if (ROOT_ONLY.has(first) || first === '*' || first.startsWith(':')) return null
  if (segments.length === 1 && TASK_KEY.test(first)) return null
  for (const [old, now] of RENAMED) {
    if (pathname === old || pathname.startsWith(`${old}/`)) return `${now}${pathname.slice(old.length)}`
  }
  return pathname
}

/**
 * Whether an old root address (a route path such as `/inbox`,
 * `/runs/:runId` or `/projects/:projectId/work-items`) is one of a
 * workspace's own pages, so it now lives under `/w/<slug>`. A task key
 * (`/AWD-12`) resolves on its own, and account settings stay at the root.
 */
export function isWorkspacePath(path: string): boolean {
  const [pathname] = split(path)
  const [first] = segmentsOf(pathname)
  if (!first) return false
  return workspaceInner(pathname) !== null
}

/** A workspace page's path inside `/w/<slug>`, from its old root path (`/projects/new` is `/spaces/new`). */
export function workspaceRoutePath(path: string): string {
  return workspaceInner(path) ?? path
}

/** The address of a workspace's page: `workspaceHref('acme', '/inbox?tab=all')` is `/w/acme/inbox?tab=all`. Takes the page's old root path or its new one. */
export function workspaceHref(ws: string, path = '/home'): string {
  const [pathname, rest] = split(path.startsWith('/') ? path : `/${path}`)
  return `/w/${encodeURIComponent(ws)}${workspaceRoutePath(pathname)}${rest}`
}

/** The full address of a page opened by a link (C1.27), to copy and send: `/p/<token>` on this server. */
export function publicPageUrl(token: string): string {
  return `${window.location.origin}/p/${encodeURIComponent(token)}`
}

/** A page of the workspace links are built in now: `here('/inbox')`. Leaves account settings and other root pages as they are. */
export function here(path = '/home'): string {
  const [pathname] = split(path.startsWith('/') ? path : `/${path}`)
  if (pathname.startsWith('/w/')) return path
  return isWorkspacePath(pathname) || pathname === '/' ? workspaceHref(currentSlug, path) : path
}

/**
 * A place in a space: `spaceHref('p1', '/tasks/t1')` is
 * `/w/<slug>/s/p1/tasks/t1`. `path` may use today's words (`/work-items`),
 * which become the address's. `ws` defaults to the workspace in view.
 */
export function spaceHref(spaceId: string, path = '', ws: string = currentSlug): string {
  const [pathname, rest] = split(path && !path.startsWith('/') && !path.startsWith('?') && !path.startsWith('#') ? `/${path}` : path)
  const tail = joined(spaceTail(segmentsOf(pathname)))
  return `/w/${encodeURIComponent(ws)}/s/${encodeURIComponent(spaceId)}${tail}${rest}`
}

/** A space's settings page: `/w/<slug>/settings/spaces/<id>/<section>`. */
export function spaceSettingsHref(spaceId: string, section = 'general', ws: string = currentSlug): string {
  return `/w/${encodeURIComponent(ws)}/settings/spaces/${encodeURIComponent(spaceId)}/${encodeURIComponent(section)}`
}

/** A page of the workspace's settings: `/w/<slug>/settings/<section>`. */
export function workspaceSettingsPageHref(section = 'general', ws: string = currentSlug): string {
  return `/w/${encodeURIComponent(ws)}/settings/${encodeURIComponent(section)}`
}


const WORKSPACE_ADDRESS = /^\/w\/([^/]+)(\/.*)?$/

/** The workspace slug in an address, or null outside `/w/`. */
export function workspaceSlugOf(pathname: string): string | null {
  const match = WORKSPACE_ADDRESS.exec(pathname)
  if (!match) return null
  try {
    return decodeURIComponent(match[1])
  } catch {
    return null
  }
}

/** What follows `/w/<slug>` in an address: `/inbox` in `/w/acme/inbox`, empty for the workspace itself. */
export function withinWorkspace(pathname: string): string {
  return WORKSPACE_ADDRESS.exec(pathname)?.[2] ?? ''
}

/** Whether `href` is an address inside a space, in either shape (a server's link may still be `/projects/<id>/...`). */
export function isSpaceAddress(href: string): boolean {
  return /^\/projects\/[^/]+\/./.test(href) || /^\/projects\/[^/?#]+$/.test(href) || /^\/w\/[^/]+\/s\/[^/]+/.test(href)
}

/** The space in an address (`/w/<slug>/s/<id>/...`, its settings, or an old `/projects/<id>/...`), or null. */
export function spaceIdOf(pathname: string): string | null {
  const match =
    /^\/w\/[^/]+\/s\/([^/]+)/.exec(pathname) ??
    /^\/w\/[^/]+\/settings\/spaces\/([^/]+)/.exec(pathname) ??
    /^\/settings\/projects\/([^/]+)/.exec(pathname) ??
    /^\/projects\/(?!new(?:\/|$))([^/]+)/.exec(pathname)
  if (!match) return null
  try {
    return decodeURIComponent(match[1])
  } catch {
    return null
  }
}

/**
 * The page's old root path, for code that reads addresses by their old
 * shape (which module owns a page, its title, which rail tile is lit):
 * `/w/acme/spaces` is `/projects`, `/w/acme/dm/1` is `/messages/1`,
 * `/w/acme/s/p1/tasks` is `/projects/p1/work-items`. Other addresses come
 * back as they are. W1.14d retires it with the old shape.
 */
export function legacyPathOf(pathname: string): string {
  const match = WORKSPACE_ADDRESS.exec(pathname)
  if (!match) return pathname
  const inner = match[2] && match[2] !== '/' ? match[2] : '/home'
  const segments = segmentsOf(inner)
  const [first, second, ...rest] = segments
  if (first === 'dm' && second) return `/messages/${segments.slice(1).join('/')}`
  if (first === 's' && second) return `/projects/${second}${joined(legacySpaceTail(rest))}`
  if (first === 'settings') {
    if (second === 'spaces') return `/settings/projects${joined(segments.slice(2))}`
    return `/settings/workspace${joined(segments.slice(1))}`
  }
  for (const [old, now] of RENAMED) {
    if (inner === now || inner.startsWith(`${now}/`)) return `${old}${inner.slice(now.length)}`
  }
  return inner
}
