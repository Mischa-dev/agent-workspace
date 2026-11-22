import { isWorkspacePath, workspaceHref } from '../src/app/href'

/**
 * A page's address from its old root path (`/inbox`, a module's registered
 * route): in the workspace used last (`/w/-/inbox`), since W1.14d removed
 * the redirects from the old roots. Account settings and other root pages
 * come back as they are.
 */
export function at(path: string): string {
  return isWorkspacePath(path) ? workspaceHref('-', path) : path
}
