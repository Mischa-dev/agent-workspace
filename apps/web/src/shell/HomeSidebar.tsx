import { useHomeExperience } from '../experience/layout'
import { rowLabel, sidebarRows } from '../modules/registry'
import { useInboxUnread } from './homeSpace'
import { SidebarFrame, SidebarLink } from './SidebarParts'

/** The Home space's sidebar: your inbox, work and saved things across every project. Its rows are the modules' registrations (M1.4) and follow the workspace's layout. */
export function HomeSidebar({ mobileOpen, onNavigate }: { mobileOpen: boolean; onNavigate: () => void }) {
  const inboxUnread = useInboxUnread() ?? 0
  const experience = useHomeExperience()
  const badges = { inbox: inboxUnread }
  return (
    <SidebarFrame id="project-sidebar" label="Home navigation" mobileOpen={mobileOpen} space="home" title="Home">
      <ul className="aw-section__list aw-sidebar__top">
        <SidebarLink end icon="home" label="Home" onNavigate={onNavigate} to="/home" />
        {sidebarRows('home', experience).map((row) => (
          <SidebarLink
            badge={row.badge ? badges[row.badge] : undefined}
            icon={row.icon}
            key={`${row.module}:${row.id}`}
            label={rowLabel(row, experience)}
            onNavigate={onNavigate}
            to={row.path}
          />
        ))}
      </ul>
    </SidebarFrame>
  )
}
