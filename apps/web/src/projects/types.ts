/** The project's places with an address of their own and nothing to name. */
export type PlainProjectPlace =
  | 'project_overview'
  | 'work_items'
  | 'settings'
  | 'milestones'
  | 'timeline'
  | 'planning'
  | 'pages'
  | 'decisions'
  | 'runs'

export type ProjectLocation =
  | { kind: PlainProjectPlace }
  | { kind: 'channel'; conversationId: string }
  | { kind: 'thread'; conversationId: string }
  | { kind: 'page'; pageId: string }

export type ProjectCapabilities = {
  view: boolean
  manage: boolean
  archive: boolean
  navigationWrite: boolean
}

export type ProjectNavigation = {
  location: ProjectLocation
  href: string
  pinned: boolean
  lastOpenedAt: string | null
  version: number
  contextDrawerOpen?: boolean
  filters?: Record<string, unknown>
}

export type Project = {
  id: string
  organizationId: string
  key: string
  name: string
  archivedAt: string | null
  role: string
  version: number
  attentionCount: number
  capabilities: ProjectCapabilities
  navigation: ProjectNavigation
}

export type ProjectListResponse = {
  projects: Project[]
}

export type ProjectSwitchResponse = {
  project: Project
  navigation: ProjectNavigation
  href: string
}
