import type { Query } from '@tanstack/react-query'

import { aiKeys } from '../../ai/keys'
import { queryKeys } from '../../api/queryKeys'
import { dashboards, myWork, payloadId, references, thing, type Handler } from './touches'

// Tasks' events. A changed task is patched into lists, boards and its detail
// in place (workItemPatches.ts); these are the other places that show it.

type TimelineData = {
  linkedWorkItems?: Array<{ id?: unknown }>
  entries?: Array<{ message?: { referencedWorkItems?: Array<{ id?: unknown }> } | null }>
}

/** Whether a cached conversation timeline shows the task as a chip (linked, or referenced in a message). */
function showsTask(query: Query, projectId: string, workItemId: string) {
  const key = query.queryKey
  const timeline = queryKeys.timeline(projectId, '')
  if (key[0] !== timeline[0] || key[1] !== projectId || key[2] !== timeline[2] || key[4] !== timeline[4]) return false
  const data = query.state.data as TimelineData | undefined
  return Boolean(
    data?.linkedWorkItems?.some((chip) => chip.id === workItemId) ||
      data?.entries?.some((entry) => entry.message?.referencedWorkItems?.some((reference) => reference.id === workItemId)),
  )
}

const workItemUpdated: Handler = (event) => {
  const workItemId = payloadId(event, 'workItemId', 'work_item_id')
  if (!workItemId) return []
  const projectId = event.projectId
  return [
    { id: `task-timeline:${workItemId}`, filters: { queryKey: queryKeys.workItemTimeline(projectId, workItemId) } },
    myWork,
    dashboards,
    // A parent's overview counts the tasks of the projects inside it.
    { id: 'project-trees', filters: { queryKey: queryKeys.projectTrees } },
    thing('task', workItemId),
    // Its description or comments may have added or dropped a link.
    references,
    // Planning ranks, graphs and checks readiness from tasks.
    { id: `ai:${projectId}`, filters: { queryKey: aiKeys.root(projectId) } },
    { id: `command-menu:${projectId}`, filters: { queryKey: queryKeys.commandMenu(projectId) } },
    { id: `timelines-showing:${workItemId}`, filters: { predicate: (query) => showsTask(query, projectId, workItemId) } },
  ]
}

export const taskHandlers: Record<string, Handler> = {
  'work_item.updated': workItemUpdated,
}
