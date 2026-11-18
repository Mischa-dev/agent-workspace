import { NavLink } from 'react-router-dom'

import { spaceHref } from '../app/href'
import { useWorkspaceHref } from '../app/workspace'
import { useHomeExperience } from '../experience/layout'
import { phoneTabs, rowLabel } from '../modules/registry'
import { useProjects } from '../projects/hooks'
import type { Project } from '../projects/types'
import { Icon } from '../ui'
import { useInboxUnread } from './homeSpace'
import './phoneTabs.css'

/** The space a space's tab opens: the one you're in, else the one you opened last. */
function lastSpace(projects: readonly Project[]) {
  let latest: Project | null = null
  for (const project of projects) {
    if (project.archivedAt) continue
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
  const space = spaceId ?? lastSpace(projects.data?.projects ?? [])?.id ?? null
  const tabs = phoneTabs(experience).flatMap((tab) => {
    if (tab.space === 'home') return [{ ...tab, to: href(tab.path), end: false }]
    return space ? [{ ...tab, to: spaceHref(space, tab.path), end: !tab.path }] : []
  })
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
            <span>{rowLabel(tab, experience)}</span>
            {count > 0 ? (
              <span className="aw-phonetabs__badge" aria-label={`${count} unread`}>
                {count > 99 ? '99+' : count}
              </span>
            ) : null}
          </NavLink>
        )
      })}
    </nav>
  )
}
