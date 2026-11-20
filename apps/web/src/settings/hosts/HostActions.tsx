import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'

import { describeError } from '../../app/mutations'
import { opQuery, useOp } from '../../api/ops'
import { HostPanel } from '../../external-agents/HostPanel'
import { Button, ConfirmDialog } from '../../ui'

/** An action a host publishes for itself or for one of its agents (R21). */
export type HostAction = { id: string; label: string; needs?: 'confirm' | 'panel'; danger?: boolean }

const SETTLED = new Set(['acked', 'refused', 'expired', 'superseded'])

/**
 * A2.19: a host's actions as buttons. Workspace has no list of its own:
 * whatever the host publishes is drawn, a `danger` one asks first, and the
 * host's reply is shown once it answers. When the host is joined to its
 * connected module and its answer names one of the module's panels (M1.16),
 * that panel opens instead.
 */
export function HostActions({
  actions,
  account,
  hostId,
  workspaceId,
}: {
  actions: HostAction[]
  /** The agent the actions are for; left out, the host's own. */
  account?: string
  hostId: string
  workspaceId: string | null
}) {
  const where = workspaceId ?? undefined
  const [confirming, setConfirming] = useState<HostAction | null>(null)
  const [commandId, setCommandId] = useState<string | null>(null)
  // The action whose answer is awaited, and whether its panel was closed.
  const [sent, setSent] = useState<{ label: string; panelClosed: boolean } | null>(null)
  const run = useOp('agent_action_run', {
    workspaceId: where,
    onSuccess: (result) => {
      setConfirming(null)
      setCommandId(result.commandId)
    },
  })
  const reply = useQuery({
    ...opQuery('agent_action_get', { commandId: commandId ?? '' }, { workspaceId: where }),
    enabled: commandId !== null,
    refetchInterval: (query) => (query.state.data && SETTLED.has(query.state.data.state) ? false : 2000),
  })
  if (actions.length === 0) return null
  const send = (action: HostAction) => {
    setSent({ label: action.label, panelClosed: false })
    run.mutate(account ? { account, actionId: action.id } : { host: hostId, actionId: action.id })
  }
  const panel = reply.data?.panel ?? (run.data?.commandId === commandId ? run.data?.panel : null) ?? null
  const answer = reply.data?.reply as { message?: unknown; reason?: unknown } | null | undefined
  const said = typeof answer?.message === 'string' ? answer.message : typeof answer?.reason === 'string' ? answer.reason : null
  return (
    <span className="aw-row__metaline aw-wrap">
      {actions.map((action) => (
        <Button
          busy={run.isPending && run.variables?.actionId === action.id && !confirming}
          disabled={run.isPending}
          key={action.id}
          onClick={() => (action.danger || action.needs === 'confirm' ? setConfirming(action) : send(action))}
          size="sm"
          variant="ghost"
        >
          {action.label}
        </Button>
      ))}
      {commandId && panel && sent && !sent.panelClosed && (
        <HostPanel
          onClose={() => setSent({ ...sent, panelClosed: true })}
          panel={panel}
          title={sent.label}
          workspaceId={where}
        />
      )}
      {commandId && reply.data && (!panel || sent?.panelClosed) && (
        <span className="aw-quiet" role="status">
          {SETTLED.has(reply.data.state)
            ? (said ?? (reply.data.state === 'acked' ? 'Done.' : 'The host didn’t do it.'))
            : 'Waiting for the host…'}
        </span>
      )}
      {run.isError && !confirming && <span className="aw-quiet">{describeError(run.error).message}</span>}
      <ConfirmDialog
        busy={run.isPending}
        confirmLabel={confirming?.label ?? 'Run'}
        error={run.isError ? describeError(run.error).message : undefined}
        onClose={() => {
          setConfirming(null)
          run.reset()
        }}
        onConfirm={() => confirming && send(confirming)}
        open={confirming !== null}
        title={`${confirming?.label ?? 'Run it'}?`}
        tone={confirming?.danger ? 'danger' : 'primary'}
      >
        The host carries it out and says how it went.
      </ConfirmDialog>
    </span>
  )
}
