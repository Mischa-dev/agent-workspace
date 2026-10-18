import type { Project } from '../projects/types'

/** The pages in the Home space: what's yours across every Project. */
export const homeSpacePaths = ['/home', '/inbox', '/my-work', '/later', '/runs', '/whiteboards'] as const

export function inHomeSpace(pathname: string) {
  return homeSpacePaths.some((path) => pathname === path || pathname.startsWith(`${path}/`))
}

/** Every notification belongs to a Project, so the inbox's unread count is the sum. */
export function inboxUnreadCount(projects: Project[] | undefined) {
  return (projects ?? []).reduce((sum, project) => sum + project.attentionCount, 0)
}
