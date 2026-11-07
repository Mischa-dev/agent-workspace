import type { AuthenticatedSession, OrganizationSummary } from '../auth/session'
import type { Experience } from '../experience/layout'
import type { Project } from '../projects/types'

export type SettingsGroup = 'account' | 'workspace' | 'project'

/** What every settings page is given. */
export type SettingsPageProps = {
  session: AuthenticatedSession
  /** The workspace the Workspace group is about (the default one), or the Project's workspace. */
  workspace: OrganizationSummary | null
  /** Set on Project pages: the Project picked at the top of the Project group. */
  project: Project | null
  /** The person's Projects, for pickers. */
  projects: Project[]
}

/** What a page's visibility check can look at. Pages that can't be used are left out of the nav. */
export type SettingsVisibility = {
  session: AuthenticatedSession
  workspace: OrganizationSummary | null
  project: Project | null
  projects: Project[]
  /** What the person sees there: a page for a part the layout hides stays out of the nav, though links and search still reach it. */
  experience?: Experience
  /** AWD-75: which modules are on there, for a group without a layout of its own (account, workspace): its pages of an off module leave the nav. */
  modules?: Pick<Experience, 'moduleOn'>
}
