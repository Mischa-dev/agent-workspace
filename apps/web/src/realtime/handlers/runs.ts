import { runKeys } from '../../runs/keys'
import { conversation, payloadId, type Handler, type Touch } from './touches'

// Runs' events. A run's own page and the run lists refetch; a run started
// from a conversation also shows as a card in that conversation's timeline.

/** Changes that move what a run cost. */
const USAGE_CHANGES = new Set(['usage_updated', 'finished'])

const runDetail = (runId: string): Touch => ({ id: `run:${runId}`, filters: { queryKey: runKeys.detail(runId) } })
const runLists: Touch = { id: 'runs', filters: { queryKey: runKeys.lists } }

/** run.changed: any run, whoever runs it (A1's stopgap until K3's run.* events). */
const runChanged: Handler = (event) => {
  const runId = payloadId(event, 'runId', 'run_id')
  if (!runId) return []
  const change = typeof event.payload.change === 'string' ? event.payload.change : ''
  return [
    runDetail(runId),
    runLists,
    ...(USAGE_CHANGES.has(change) ? [{ id: 'runs:usage', filters: { queryKey: runKeys.usages } }] : []),
    ...(event.conversationId ? [conversation(event.projectId, event.conversationId)] : []),
  ]
}

/** conversation.agent_run.updated: an agent's reply run in a conversation. */
const agentRunUpdated: Handler = (event) => {
  const runId = payloadId(event, 'agentRunId', 'agent_run_id')
  return [
    ...(event.conversationId ? [conversation(event.projectId, event.conversationId)] : []),
    ...(runId ? [runDetail(runId), runLists] : []),
  ]
}

export const runHandlers: Record<string, Handler> = {
  'run.changed': runChanged,
  'conversation.agent_run.updated': agentRunUpdated,
}
