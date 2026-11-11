import { type FormEvent, useMemo, useState } from 'react'

import { requestJson } from '../api/client'
import { queryKeys } from '../api/queryKeys'
import { useInstantMutation } from '../app/mutations'
import { isSafeMessageText } from '../conversations/messageContentModel'
import type { ConversationTimelineEntry } from '../conversations/types'
import type { Timeline } from './threadTypes'

// What a run action failed to do, for its error toast.
const ACTION_ERRORS: Record<string, string> = {
  start: 'The run wasn’t started',
  interrupt: 'The run wasn’t stopped',
  retry: 'The run wasn’t restarted',
  publish: 'The branch wasn’t published',
  'pull-request': 'The pull request wasn’t opened',
  'pull-request-refresh': 'GitHub status wasn’t refreshed',
  'acknowledge-failure': 'The failure wasn’t dismissed',
}

const post = (path: string, idempotencyKey: string, json: unknown = {}) =>
  requestJson(path, { method: 'POST', idempotencyKey, json })

/** The thread's run commands, one request each. */
const threadApi = {
  startRun: (threadId: string, prompt: string, key: string) =>
    post(`/api/v1/threads/${encodeURIComponent(threadId)}/agent-runs`, key, { prompt }),
  respond: (approvalId: string, action: string, response: { decision: 'approve' | 'deny' } | { answer: string }) =>
    post(`/api/v1/agent-approvals/${encodeURIComponent(approvalId)}/respond`, `approval-${approvalId}-${action}`, response),
  interrupt: (runId: string) => post(`/api/v1/agent-runs/${encodeURIComponent(runId)}/interrupt`, `interrupt-${runId}`),
  retry: (runId: string) => post(`/api/v1/agent-runs/${encodeURIComponent(runId)}/retry`, `retry-${runId}`),
  publish: (projectId: string, runId: string, key: string) =>
    post(`/api/v1/projects/${encodeURIComponent(projectId)}/agent-runs/${encodeURIComponent(runId)}/publish`, key),
  createPullRequest: (projectId: string, runId: string, key: string, json: { title: string; body: string; workItemIds: string[] }) =>
    post(`/api/v1/projects/${encodeURIComponent(projectId)}/agent-runs/${encodeURIComponent(runId)}/pull-request`, key, json),
  refreshPullRequest: (projectId: string, pullRequestId: string) =>
    post(`/api/v1/projects/${encodeURIComponent(projectId)}/pull-requests/${encodeURIComponent(pullRequestId)}/refresh`, crypto.randomUUID()),
  acknowledgeFailure: (runId: string, eventId: string) =>
    post(`/api/v1/agent-runs/${encodeURIComponent(runId)}/acknowledge-failure`, `acknowledge-failure-${runId}-${eventId}`, { eventId }),
}

/**
 * Everything the thread can do to its runs: start one, answer an approval,
 * stop, retry, publish, open or refresh a pull request and dismiss a failure.
 * Each command keeps its idempotency key until it succeeds, so a retry after
 * a dropped response can't start a second run.
 */
export function useThreadRunActions({
  entries,
  includeArchived,
  projectId,
  threadId,
  timeline: verifiedTimeline,
}: {
  entries: ConversationTimelineEntry[]
  includeArchived: boolean
  projectId: string
  threadId: string
  timeline: Timeline | undefined
}) {
  const activeRun = verifiedTimeline?.activeRun ?? null
  const latestTerminalRun = verifiedTimeline?.latestTerminalRun ?? null
  const [prompt, setPrompt] = useState('')
  const [startIdempotencyKey, setStartIdempotencyKey] = useState<string | null>(
    null,
  )
  const [publishIdempotencyKey, setPublishIdempotencyKey] = useState<
    string | null
  >(null)
  const [pullRequestIdempotencyKey, setPullRequestIdempotencyKey] = useState<
    string | null
  >(null)
  const [acknowledgedRunId, setAcknowledgedRunId] = useState<string | null>(null)
  const latestFailureEvent = useMemo(
    () =>
      latestTerminalRun?.state === 'failed'
        ? [...entries]
            .reverse()
            .find(
              (entry) =>
                entry.workerEvent?.agentRunId === latestTerminalRun.id &&
                entry.workerEvent.kind === 'run_failed',
            )?.workerEvent ?? null
        : null,
    [entries, latestTerminalRun],
  )
  const action = useInstantMutation({
    mutationFn: ({ request }: { key: string; request: () => Promise<unknown> }) => request(),
    errorTitle: ({ key }) => ACTION_ERRORS[key] ?? (key.includes(':') ? 'Your answer wasn’t sent' : 'The request wasn’t sent'),
    invalidate: () => [queryKeys.timeline(projectId, threadId, includeArchived)],
    onSuccess: (_result, { key }) => {
      if (key === 'start') setPrompt('')
    },
  })
  const busyAction = action.isPending ? action.variables.key : null
  const perform = (key: string, request: () => Promise<unknown>) =>
    action.mutateAsync({ key, request }).then(
      () => true,
      () => false,
    )
  const startRun = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (verifiedTimeline?.capabilities.workerInvoke !== true || activeRun) {
      return
    }
    const value = prompt.trim()
    if (!value || !isSafeMessageText(value)) return
    const commandKey = startIdempotencyKey ?? crypto.randomUUID()
    if (!startIdempotencyKey) setStartIdempotencyKey(commandKey)
    void perform('start', () => threadApi.startRun(threadId, value, commandKey)).then((succeeded) => {
      if (succeeded) setStartIdempotencyKey(null)
    })
  }

  const respond = (
    approvalId: string,
    response: { decision: 'approve' | 'deny' } | { answer: string },
  ) => {
    const approval = verifiedTimeline?.approvals.find(
      (candidate) => candidate.id === approvalId,
    )
    if (
      !approval ||
      approval.status !== 'pending' ||
      approval.capabilities.respond !== true
    ) {
      return
    }
    const kind = 'decision' in response ? response.decision : 'answer'
    void perform(`${approvalId}:${kind}`, () => threadApi.respond(approvalId, kind, response))
  }

  const interrupt = () => {
    const runId = activeRun?.id
    if (!runId || activeRun.capabilities.interrupt !== true) return
    void perform('interrupt', () => threadApi.interrupt(runId))
  }

  const retryLastRun = () => {
    const runId = latestTerminalRun?.id
    if (!runId || latestTerminalRun.capabilities.retry !== true || activeRun) return
    void perform('retry', () => threadApi.retry(runId))
  }

  const publishLastRun = () => {
    const runId = latestTerminalRun?.id
    if (
      !runId ||
      !latestTerminalRun.repositoryWorkspaceId ||
      latestTerminalRun.capabilities.publish !== true ||
      activeRun
    ) {
      return
    }
    const commandKey = publishIdempotencyKey ?? crypto.randomUUID()
    if (!publishIdempotencyKey) setPublishIdempotencyKey(commandKey)
    void perform('publish', () => threadApi.publish(projectId, runId, commandKey)).then((succeeded) => {
      if (succeeded) setPublishIdempotencyKey(null)
    })
  }

  const createPullRequest = () => {
    const runId = latestTerminalRun?.id
    if (
      !runId ||
      latestTerminalRun.capabilities.createPullRequest !== true ||
      activeRun
    ) {
      return
    }
    // Archived tasks stay on the thread as chips, but the server refuses them
    // on a new pull request, so only active ones go with it.
    const activeWorkItems = verifiedTimeline?.linkedWorkItems.filter((item) => !item.archivedAt) ?? []
    const firstWorkItem = activeWorkItems[0]
    const rawTitle = firstWorkItem
      ? firstWorkItem.title
        ? `${firstWorkItem.key}: ${firstWorkItem.title}`
        : firstWorkItem.key
      : verifiedTimeline?.conversation.name || 'Agent Workspace change'
    const title = Array.from(rawTitle).slice(0, 256).join('')
    const commandKey = pullRequestIdempotencyKey ?? crypto.randomUUID()
    if (!pullRequestIdempotencyKey) setPullRequestIdempotencyKey(commandKey)
    void perform('pull-request', () =>
      threadApi.createPullRequest(projectId, runId, commandKey, {
        title,
        body: 'Created from an explicitly published Agent Workspace run.',
        workItemIds: activeWorkItems.map((item) => item.id),
      }),
    ).then((succeeded) => {
      if (succeeded) setPullRequestIdempotencyKey(null)
    })
  }

  const refreshPullRequest = () => {
    const pullRequestId = latestTerminalRun?.pullRequestId
    if (
      !pullRequestId ||
      latestTerminalRun.capabilities.refreshPullRequest !== true
    ) {
      return
    }
    void perform('pull-request-refresh', () => threadApi.refreshPullRequest(projectId, pullRequestId))
  }

  const acknowledgeFailure = () => {
    if (
      !latestTerminalRun ||
      !latestFailureEvent ||
      latestTerminalRun.failureAcknowledged ||
      acknowledgedRunId === latestTerminalRun.id
    ) {
      return
    }
    const runId = latestTerminalRun.id
    const eventId = latestFailureEvent.id
    void perform('acknowledge-failure', () => threadApi.acknowledgeFailure(runId, eventId)).then((succeeded) => {
      if (succeeded) setAcknowledgedRunId(runId)
    })
  }

  const editPrompt = (value: string) => {
    setPrompt(value)
    setStartIdempotencyKey(null)
  }

  return {
    acknowledgeFailure,
    acknowledgedRunId,
    busyAction,
    createPullRequest,
    editPrompt,
    interrupt,
    latestFailureEvent,
    prompt,
    publishLastRun,
    refreshPullRequest,
    respond,
    retryLastRun,
    startRun,
  }
}

export type ThreadRunActions = ReturnType<typeof useThreadRunActions>
