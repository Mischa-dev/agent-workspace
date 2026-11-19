import { NavLink } from 'react-router-dom'

import { spaceHref } from '../app/href'
import { useWorkspaceHref, useWorkspaceId } from '../app/workspace'
import { useHomeExperience, useProjectExperience } from '../experience/layout'
import { rowLabel } from '../modules/registry'
import { phoneTabs } from './phoneTabsModel'
import { useProjects } from '../projects/hooks'
import type { Project } from '../projects/types'
import { Badge, Icon } from '../ui'
import { useInboxUnread } from './homeSpace'
import './phoneTabs.css'

/** The space a space's tab opens: the one you're in, else the one you opened last. */
function lastSpace(projects: readonly Project[], workspaceId: string | null) {
  let latest: Project | null = null
  for (const project of projects) {
    if (project.archivedAt || (workspaceId && project.organizationId !== workspaceId)) continue
    if (!latest || (project.navigation.lastOpenedAt ?? '') > (latest.navigation.lastOpenedAt ?? '')) latest = project
  }
  return latest
}

/**
 * The phone's bottom bar (W1.29): Home, then one destination from each
 * module that's on and says where it goes on a phone. Only drawn at phone
 * widths; the rail and sidebar stay in the menu drawer.
 */
export default function PhoneTabs({ spaceId }: { spaceId: string | null }) {
  const experience = useHomeExperience()
  const href = useWorkspaceHref()
  const projects = useProjects()
  const inboxUnread = useInboxUnread() ?? 0
  const workspaceId = useWorkspaceId()
  const space = spaceId ?? lastSpace(projects.data?.projects ?? [], workspaceId)?.id ?? null
  // A space's destinations follow that space's layout; Home's, the workspace's.
  const spaceExperience = useProjectExperience(space)
  const tabs = [
    ...phoneTabs(experience)
      .filter((tab) => tab.space === 'home')
      .map((tab) => ({ ...tab, to: href(tab.path), end: false, name: rowLabel(tab, experience) })),
    ...(space
      ? phoneTabs(spaceExperience)
          .filter((tab) => tab.space !== 'home')
          .map((tab) => ({ ...tab, to: spaceHref(space, tab.path), end: !tab.path, name: rowLabel(tab, spaceExperience) }))
      : []),
  ].sort((left, right) => left.rank - right.rank)
  return (
    <nav aria-label="Main" className="aw-phonetabs" data-part="phone-tabs">
      <NavLink className="aw-phonetabs__tab" end to={href('/home')}>
        <Icon name="home" size={20} />
        <span>Home</span>
      </NavLink>
      {tabs.map((tab) => {
        const count = tab.badge === 'inbox' ? inboxUnread : 0
        return (
          <NavLink className="aw-phonetabs__tab" end={tab.end} key={`${tab.module}:${tab.id}`} to={tab.to}>
            <Icon name={tab.icon} size={20} />
            <span>{tab.name}</span>
            <Badge className="aw-phonetabs__badge" count={count} label={`${count} unread`} />
          </NavLink>
        )
      })}
    </nav>
  )
}
