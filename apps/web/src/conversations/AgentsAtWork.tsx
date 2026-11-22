import { useState } from 'react'

import { actorKind, AvatarStack, Button, describeError, Dots } from '../ui'
import type { ReplyRequest } from './types'
import '../styles/conversation.css'

const ACTIVE = new Set<ReplyRequest['status']>(['pending', 'claimed'])

/** "Mara", "Mara and Rook", "Mara, Rook and 2 others". */
function agentsSentence(names: string[]): string {
  if (names.length <= 1) return names[0] ?? ''
  if (names.length === 2) return `${names[0]} and ${names[1]}`
  if (names.length === 3) return `${names[0]}, ${names[1]} and ${names[2]}`
  return `${names[0]}, ${names[1]} and ${names.length - 2} others`
}

/**
 * C2.35: when more than one agent is at work in a conversation, one row says
 * who, and Stop all stops every one of them (`reply_stop_all`). Each agent's
 * own line under the message it answers keeps its own Stop.
 */
export function AgentsAtWork({
  onStopAll,
  requests,
}: {
  /** Without it, no Stop all (people who can't post here can't stop agents). */
  onStopAll?: () => Promise<unknown>
  requests: ReplyRequest[]
}) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const agents = new Map<string, ReplyRequest['agent']>()
  let working = false
  for (const request of requests) {
    if (!ACTIVE.has(request.status)) continue
    agents.set(request.agent.id, request.agent)
    working ||= request.status === 'claimed'
  }
  if (agents.size < 2) return null
  const people = [...agents.values()]
  const stopAll = () => {
    if (!onStopAll) return
    setBusy(true)
    setError(null)
    onStopAll()
      .catch((reason: unknown) => setError(`They weren’t stopped. ${describeError(reason).message}`))
      .finally(() => setBusy(false))
  }
  return (
    <div className="aw-agents-at-work" role="status">
      <AvatarStack max={3} people={people.map((agent) => ({ id: agent.id, kind: actorKind(agent.kind), name: agent.displayName }))} size={20} />
      <span className="aw-agents-at-work__text">
        {agentsSentence(people.map((agent) => agent.displayName))} {working ? 'are working on it' : 'are asked to reply'}
      </span>
      {working ? <Dots /> : null}
      {onStopAll ? (
        <Button busy={busy} icon="stop" onClick={stopAll} size="sm" variant="ghost">
          Stop all
        </Button>
      ) : null}
      {error ? (
        <span className="aw-agents-at-work__error" role="alert">
          {error}
        </span>
      ) : null}
    </div>
  )
}
