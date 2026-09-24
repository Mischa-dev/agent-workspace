import { NavLink } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { conversationTitle, useDirectOverview } from './api'

export function DirectSidebar({
  mobileOpen,
  onNavigate,
}: {
  mobileOpen: boolean
  onNavigate: () => void
}) {
  const session = useAuthenticatedSession()
  const overview = useDirectOverview()
  const conversations = overview.data?.conversations ?? []
  return (
    <aside
      aria-label="Messages navigation"
      className="project-sidebar"
      data-mobile-open={mobileOpen}
      id="project-sidebar"
    >
      <header className="project-heading">
        <span className="section-label">Direct</span>
        <p>Messages</p>
      </header>
      <nav aria-label="Messages">
        <ul>
          <li>
            <NavLink end onClick={onNavigate} to="/messages">
              <span aria-hidden="true">＋</span>
              New message
            </NavLink>
          </li>
          {overview.isPending ? (
            <li>
              <span className="rail-loading">Loading…</span>
            </li>
          ) : null}
          {conversations.map((conversation) => (
            <li className="channel-navigation" key={conversation.id}>
              <NavLink
                onClick={onNavigate}
                to={`/messages/${encodeURIComponent(conversation.id)}`}
              >
                <span aria-hidden="true">
                  {conversation.kind === 'dm' ? '@' : '◎'}
                </span>
                <span>
                  {conversationTitle(conversation, session.user.actorId)}
                </span>
                {conversation.unread ? (
                  <span aria-label="Unread Messages" className="unread-dot">
                    ●
                  </span>
                ) : null}
              </NavLink>
            </li>
          ))}
          {overview.data?.capabilities.manageAgents ? (
            <li>
              <NavLink onClick={onNavigate} to="/messages/agents">
                <span aria-hidden="true">⚙</span>
                Agents
              </NavLink>
            </li>
          ) : null}
        </ul>
      </nav>
    </aside>
  )
}
