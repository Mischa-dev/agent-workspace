import type { ReactNode } from 'react'

import { isSafeMessageText, messageTextScalarValues } from '../conversations/messageContentModel'
import { Button, Icon } from '../ui'
import { runAgent } from './runAgent'
import type { Run } from './threadTypes'
import type { ThreadRunActions } from './useThreadRunActions'

/** The buttons under the run card: stop the active run, or follow up on the last one. */
export function RunCardActions({
  actions,
  activeRun,
  latestTerminalRun,
}: {
  actions: ThreadRunActions
  activeRun: Run | null
  latestTerminalRun: Run | null
}) {
  const { acknowledgedRunId, busyAction, latestFailureEvent } = actions
  const busy = Boolean(busyAction)
  if (activeRun) {
    return activeRun.capabilities.interrupt ? (
      <Button busy={busyAction === 'interrupt'} icon="stop" onClick={actions.interrupt} size="sm">
        {busyAction === 'interrupt' ? 'Stopping…' : 'Stop run'}
      </Button>
    ) : null
  }
  return (
    <>
      {latestTerminalRun?.capabilities.createPullRequest ? (
        <Button busy={busyAction === 'pull-request'} disabled={busy} icon="pull-request" onClick={actions.createPullRequest} size="sm" variant="primary">
          {busyAction === 'pull-request' ? 'Opening pull request…' : 'Create pull request'}
        </Button>
      ) : null}
      {latestTerminalRun?.capabilities.publish && latestTerminalRun.repositoryWorkspaceId ? (
        <Button busy={busyAction === 'publish'} disabled={busy} icon="branch" onClick={actions.publishLastRun} size="sm">
          {busyAction === 'publish' ? 'Publishing…' : 'Publish branch'}
        </Button>
      ) : null}
      {latestTerminalRun?.capabilities.retry ? (
        <Button busy={busyAction === 'retry'} disabled={busy} icon="retry" onClick={actions.retryLastRun} size="sm">
          {busyAction === 'retry' ? 'Starting again…' : 'Run again'}
        </Button>
      ) : null}
      {latestTerminalRun?.pullRequestId && latestTerminalRun.capabilities.refreshPullRequest ? (
        <Button busy={busyAction === 'pull-request-refresh'} disabled={busy} icon="retry" onClick={actions.refreshPullRequest} size="sm" variant="ghost">
          {busyAction === 'pull-request-refresh' ? 'Checking GitHub…' : 'Refresh GitHub status'}
        </Button>
      ) : null}
      {latestFailureEvent && !latestTerminalRun?.failureAcknowledged && acknowledgedRunId !== latestTerminalRun?.id ? (
        <Button busy={busyAction === 'acknowledge-failure'} disabled={busy} onClick={actions.acknowledgeFailure} size="sm" variant="ghost">
          {busyAction === 'acknowledge-failure' ? 'Dismissing…' : 'Dismiss failure'}
        </Button>
      ) : null}
    </>
  )
}

/** What still needs attention after the last run: publishing, the pull request, GitHub sync. */
export function RunCardNotes({ run }: { run: Run }) {
  return (
    <>
      {run.publishFailureCategory ? (
        <p className="aw-run__error" role="status">
          <Icon name="circle-alert" size={14} />
          Publishing needs attention: {run.publishFailureCategory.replaceAll('_', ' ')}.
        </p>
      ) : null}
      {run.pullRequestRequestState && !run.pullRequestUrl ? (
        <p className="aw-run__detail">Pull request: {run.pullRequestRequestState.replaceAll('_', ' ')}</p>
      ) : null}
      {run.pullRequestFailureCategory ? (
        <p className="aw-run__error" role="status">
          <Icon name="circle-alert" size={14} />
          The pull request needs attention: {run.pullRequestFailureCategory.replaceAll('_', ' ')}.
        </p>
      ) : null}
      {run.pullRequestSyncState === 'failed' ? (
        <p className="aw-run__error" role="status">
          <Icon name="circle-alert" size={14} />
          GitHub status needs attention: {(run.pullRequestSyncFailureCategory ?? 'sync_failed').replaceAll('_', ' ')}.
        </p>
      ) : null}
      {run.pullRequestAutomationState && run.pullRequestAutomationState !== 'not_evaluated' ? (
        <p className="aw-run__detail">
          Work item automation: {run.pullRequestAutomationState.replaceAll('_', ' ')}
          {run.pullRequestAutomationReason ? ` · ${run.pullRequestAutomationReason.replaceAll('_', ' ')}` : ''}
        </p>
      ) : null}
    </>
  )
}

/** The composer in run mode: describe a change and start a run in this thread. */
export function RunComposer({
  actions,
  activeRun,
  modeSwitch,
}: {
  actions: ThreadRunActions
  activeRun: Run | null
  modeSwitch: ReactNode
}) {
  const { busyAction, prompt } = actions
  const working = activeRun ? runAgent(activeRun).name : null
  return (
    <form className="aw-composer-wrap" onSubmit={actions.startRun}>
      {modeSwitch ? <div className="aw-composer__toolbar">{modeSwitch}</div> : null}
      <div className="aw-composer aw-composer--run">
        <label className="visually-hidden" htmlFor="agent-prompt">
          Describe the change for the agent
        </label>
        <textarea
          disabled={Boolean(activeRun) || busyAction === 'start'}
          id="agent-prompt"
          onChange={(event) => actions.editPrompt(event.target.value)}
          placeholder={
            working ? `${working} is already working in this thread. Stop the run to start a new one.` : 'Describe the change for an agent to make…'
          }
          rows={3}
          value={prompt}
        />
        <div className="aw-composer__bar">
          <span className="aw-composer__meta">
            <Icon name="terminal" size={14} /> Runs in an isolated workspace on your host
          </span>
          <div className="aw-composer__end">
            {messageTextScalarValues(prompt) > 18_000 ? (
              <span className="aw-composer__count">{(20_000 - messageTextScalarValues(prompt)).toLocaleString()}</span>
            ) : null}
            <Button
              busy={busyAction === 'start'}
              disabled={Boolean(activeRun) || !prompt.trim() || !isSafeMessageText(prompt.trim())}
              icon="terminal"
              size="sm"
              type="submit"
              variant="primary"
            >
              {busyAction === 'start' ? 'Starting…' : 'Start run'}
            </Button>
          </div>
        </div>
      </div>
      <p className="aw-composer__hint" aria-hidden="true" />
    </form>
  )
}
