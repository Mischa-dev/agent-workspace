import { useProjects } from '../projects/hooks'
import { inboxUnreadCount } from './homeSpace'
import { SidebarFrame, SidebarLink } from './SidebarParts'

/** The Home space's sidebar: your inbox, work and saved things across every Project. */
export function HomeSidebar({ mobileOpen, onNavigate }: { mobileOpen: boolean; onNavigate: () => void }) {
  const projects = useProjects()
  const inboxUnread = inboxUnreadCount(projects.data?.projects)
  return (
    <SidebarFrame id="project-sidebar" label="Home navigation" mobileOpen={mobileOpen} title="Home">
      <ul className="aw-section__list aw-sidebar__top">
        <SidebarLink end icon="home" label="Home" onNavigate={onNavigate} to="/home" />
        <SidebarLink badge={inboxUnread} icon="inbox" label="Inbox" onNavigate={onNavigate} to="/inbox" />
        <SidebarLink icon="checklist" label="My work" onNavigate={onNavigate} to="/my-work" />
        <SidebarLink icon="bookmark" label="Later" onNavigate={onNavigate} to="/later" />
        <SidebarLink icon="activity" label="Runs" onNavigate={onNavigate} to="/runs" />
        <SidebarLink icon="whiteboard" label="Whiteboards" onNavigate={onNavigate} to="/whiteboards" />
      </ul>
    </SidebarFrame>
  )
}
