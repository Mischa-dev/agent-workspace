// Addresses (W1.14a): a workspace's own pages live under `/w/<slug>/...`.
// Links to them are built here, never pasted; W1.14b adds spaces and things
// (`/w/<slug>/s/<space>/...`, `/w/<slug>/t/<kind>/<id>`) to the same file.

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

/** First segments that stay at the root: settings, the kit, the signed-out pages and the workspace form itself. */
const ROOT_ONLY = new Set(['settings', 'design-system', 'w', 'forms', 'sign-in', 'forgot-password', 'reset-password', 'verify-email', 'join'])

/** A task key such as `AWD-12`, which opens its task wherever it is. */
const TASK_KEY = /^[A-Za-z][A-Za-z0-9]{1,9}-[1-9][0-9]{0,8}$/

const split = (path: string) => {
  const at = path.search(/[?#]/)
  return at === -1 ? [path, ''] : [path.slice(0, at), path.slice(at)]
}

/**
 * Whether an old root address (a route path such as `/inbox` or
 * `/runs/:runId`) is one of a workspace's own pages, so it now lives under
 * `/w/<slug>`. A project's pages stay under `/projects/<id>` until W1.14b,
 * and a task key (`/AWD-12`) resolves on its own.
 */
export function isWorkspacePath(path: string): boolean {
  const segments = split(path)[0].split('/').filter(Boolean)
  const [first, second] = segments
  if (!first || first.startsWith(':') || first === '*' || ROOT_ONLY.has(first)) return false
  if (segments.length === 1 && TASK_KEY.test(first)) return false
  if (first === 'projects') return second === undefined || second === 'new'
  return true
}

const renamed = (pathname: string, from: 0 | 1) => {
  for (const pair of RENAMED) {
    const [old, now] = [pair[from], pair[1 - from]]
    if (pathname === old || pathname.startsWith(`${old}/`)) return `${now}${pathname.slice(old.length)}`
  }
  return pathname
}

/** A workspace page's path inside `/w/<slug>`, from its old root path (`/projects/new` is `/spaces/new`). */
export function workspaceRoutePath(path: string): string {
  return renamed(path, 0)
}

/** The address of a workspace's page: `workspaceHref('acme', '/inbox?tab=all')` is `/w/acme/inbox?tab=all`. Takes the page's old root path or its new one. */
export function workspaceHref(ws: string, path = '/home'): string {
  const [pathname, rest] = split(path.startsWith('/') ? path : `/${path}`)
  return `/w/${encodeURIComponent(ws)}${workspaceRoutePath(pathname)}${rest}`
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

/**
 * The page's old root path, for code that still reads addresses by their old
 * shape (which module owns a page, its title, which rail tile is lit):
 * `/w/acme/spaces` is `/projects`, `/w/acme/dm/1` is `/messages/1`. Other
 * addresses come back as they are. A stopgap until W1.14b gives those readers
 * the route's handle instead.
 */
export function legacyPathOf(pathname: string): string {
  const match = WORKSPACE_ADDRESS.exec(pathname)
  if (!match) return pathname
  const inner = match[2] && match[2] !== '/' ? match[2] : '/home'
  if (inner.startsWith('/dm/')) return `/messages/${inner.slice('/dm/'.length)}`
  return renamed(inner, 1)
}
