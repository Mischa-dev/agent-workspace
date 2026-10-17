import { Icon } from '../ui'
import { useAgentSessions } from './hooks'
import './launch.css'

/** "Codex Cloud · Open session" for each agent session working in a thread that has a page to watch it on. */
export function SessionLinks({ conversationId, projectId }: { conversationId: string; projectId: string }) {
  const sessions = useAgentSessions(projectId, conversationId)
  const linked = (sessions.data?.sessions ?? []).filter((session) => session.url?.startsWith('https://') && session.state !== 'cancelled')
  if (linked.length === 0) return null
  return (
    <span className="aw-sessionlinks">
      {linked.slice(0, 2).map((session) => (
        <a
          aria-label={`Open ${session.agentName}’s ${session.harnessName} session in a new tab`}
          href={session.url ?? undefined}
          key={session.id}
          rel="noreferrer noopener"
          target="_blank"
        >
          <Icon name={session.state === 'active' ? 'loader' : 'external'} className={session.state === 'active' ? 'aw-spin' : undefined} size={14} />
          <span className="aw-sessionlinks__harness">{session.harnessName} · </span>
          Open session
        </a>
      ))}
    </span>
  )
}
