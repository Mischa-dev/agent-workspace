import { useHomeExperience } from '../experience/layout'
import { useInboxUnread } from './homeSpace'
import { SidebarFrame, SidebarLink } from './SidebarParts'

/** The Home space's sidebar: your inbox, work and saved things across every project. Its rows follow the workspace's layout. */
export function HomeSidebar({ mobileOpen, onNavigate }: { mobileOpen: boolean; onNavigate: () => void }) {
  const inboxUnread = useInboxUnread() ?? 0
  const experience = useHomeExperience()
  return (
    <SidebarFrame id="project-sidebar" label="Home navigation" mobileOpen={mobileOpen} space="home" title="Home">
      <ul className="aw-section__list aw-sidebar__top">
        <SidebarLink end icon="home" label="Home" onNavigate={onNavigate} to="/home" />
        {experience.moduleOn('inbox') ? <SidebarLink badge={inboxUnread} icon="inbox" label="Inbox" onNavigate={onNavigate} to="/inbox" /> : null}
        {experience.shows('workspace.my_work') ? <SidebarLink icon="checklist" label="My work" onNavigate={onNavigate} to="/my-work" /> : null}
        {experience.shows('workspace.later') ? <SidebarLink icon="bookmark" label="Later" onNavigate={onNavigate} to="/later" /> : null}
        {experience.shows('workspace.runs') ? <SidebarLink icon="activity" label="Runs" onNavigate={onNavigate} to="/runs" /> : null}
        {experience.shows('area.pages') ? <SidebarLink icon="list" label="Pages" onNavigate={onNavigate} to="/pages" /> : null}
        {experience.shows('area.whiteboards') ? (
          <SidebarLink icon="whiteboard" label="Whiteboards" onNavigate={onNavigate} to="/whiteboards" />
        ) : null}
      </ul>
    </SidebarFrame>
  )
}
