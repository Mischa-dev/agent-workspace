import { NavLink } from 'react-router-dom'

import { threadHref } from '../launch/launchModel'
import { SidebarSection } from '../shell/SidebarParts'
import type { AgentPresence } from './api'
import { AgentMark } from './AgentMark'
import { usePresence } from './hooks'
import { presenceLabel } from './presenceModel'
import './agents.css'

/**
 * The project's agents in its sidebar, each with what it's doing now, so you
 * can see who is working and jump to the thread. Polled, and loaded on its own.
 */
export default function SidebarAgents({ onNavigate, projectId }: { onNavigate: () => void; projectId: string }) {
  const presence = usePresence(projectId)
  const agents = presence.data?.agents ?? []
  if (!agents.length) return null
  return (
    <SidebarSection label="Agents">
      {agents.map((agent) => (
        <li key={agent.actorId}>
          <AgentRow agent={agent} onNavigate={onNavigate} projectId={projectId} />
        </li>
      ))}
    </SidebarSection>
  )
}

function AgentRow({ agent, onNavigate, projectId }: { agent: AgentPresence; onNavigate: () => void; projectId: string }) {
  const label = presenceLabel(agent)
  const body = (
    <>
      <span className="aw-item__lead aw-sideagent__lead">
        <AgentMark avatar={agent.avatar} color={agent.color} name={agent.name} seed={agent.actorId} size={20} />
        <span aria-hidden className={`aw-agentdot aw-agentdot--${agent.state} aw-sideagent__dot`} />
      </span>
      <span className="aw-item__label aw-sideagent__text">
        <span className="aw-sideagent__name">{agent.name}</span>
        <span className="aw-sideagent__state">{label}</span>
      </span>
    </>
  )
  if (!agent.conversationId) {
    return (
      <div aria-label={`${agent.name}, ${label}`} className="aw-item aw-sideagent" role="group">
        {body}
      </div>
    )
  }
  return (
    <NavLink
      aria-label={`${agent.name}, ${label}`}
      className={({ isActive }) => ['aw-item', 'aw-sideagent', isActive ? 'is-active' : ''].filter(Boolean).join(' ')}
      onClick={onNavigate}
      to={threadHref(projectId, agent.conversationId)}
    >
      {body}
    </NavLink>
  )
}
