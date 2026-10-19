import { useNavigate } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { SidebarFrame, SidebarLink, SidebarSection } from '../shell/SidebarParts'
import { Banner, IconButton, ListSkeleton, actorKind } from '../ui'
import { conversationTitle, useDirectOverview } from './api'

export function DirectSidebar({ mobileOpen, onNavigate }: { mobileOpen: boolean; onNavigate: () => void }) {
  const session = useAuthenticatedSession()
  const overview = useDirectOverview()
  const navigate = useNavigate()
  const conversations = overview.data?.conversations ?? []
  const viewerId = session.user.actorId
  return (
    <SidebarFrame
      actions={
        <IconButton
          icon="compose"
          label="New message"
          onClick={() => {
            onNavigate()
            navigate('/messages')
          }}
          side="bottom"
        />
      }
      id="project-sidebar"
      label="Messages navigation"
      mobileOpen={mobileOpen}
      title="Messages"
    >
      <SidebarSection label="Direct messages">
        {conversations.map((conversation) => {
          const other = conversation.members.find((member) => member.id !== viewerId)
          return (
            <SidebarLink
              avatar={
                conversation.kind === 'dm' && other
                  ? { name: other.displayName, kind: actorKind(other.kind), seed: other.id }
                  : undefined
              }
              icon={conversation.kind === 'group' ? 'users' : 'message'}
              keep={conversation.unread}
              key={conversation.id}
              label={conversationTitle(conversation, viewerId)}
              meta={conversation.kind === 'group' ? String(conversation.members.length) : undefined}
              onNavigate={onNavigate}
              to={`/messages/${encodeURIComponent(conversation.id)}`}
              unread={conversation.unread}
            />
          )
        })}
      </SidebarSection>
      {overview.isPending ? (
        <div className="aw-sidebar__skeleton">
          <ListSkeleton avatars label="Loading conversations" rows={4} />
        </div>
      ) : null}
      {overview.isError ? (
        <Banner className="aw-sidebar__note" onRetry={() => void overview.refetch()} tone="danger">
          Your conversations didn’t load.
        </Banner>
      ) : null}
      {overview.data && conversations.length === 0 ? (
        <p className="aw-sidebar__note">No conversations yet. Start one with New message.</p>
      ) : null}
      {overview.data?.capabilities.manageAgents ? (
        <ul className="aw-section__list aw-sidebar__bottom">
          <SidebarLink icon="bot" label="Agents" onNavigate={onNavigate} to="/messages/agents" />
        </ul>
      ) : null}
    </SidebarFrame>
  )
}
