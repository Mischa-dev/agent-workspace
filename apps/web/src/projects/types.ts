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
  /** Where this pinned project sits in your rail, from a drag; unplaced pins follow. */
  railPosition?: number | null
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
  /** An emoji shown beside the name (K4.7), when it has one. */
  icon?: string | null
  archivedAt: string | null
  role: string
  /** What the caller may do here (K1.26); ask with `can`, never the role's name. */
  permissions?: string[]
  version: number
  attentionCount: number
  /** AWD-75: the modules off for you in this project (its workspace, or your role there). */
  modulesOff?: string[]
  /** The kind of space (K4.10); `workspace` is the workspace's own space, never listed as a project. */
  kind?: SpaceKind
  /** The project this one is inside, when it is a sub-project you can see the parent of. */
  parentId?: string | null
  /** Its people are the parent's, kept in step, plus anyone added here. */
  peopleFromParent?: boolean
  capabilities: ProjectCapabilities
  navigation: ProjectNavigation
}

/** The kinds of space (K4). */
export type SpaceKind = 'project' | 'workspace' | 'team' | 'personal'

export type ProjectListResponse = {
  projects: Project[]
}

export type ProjectSwitchResponse = {
  project: Project
  navigation: ProjectNavigation
  href: string
}
