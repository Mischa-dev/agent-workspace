import { useQuery } from '@tanstack/react-query'

import { callOp, type OpOutput } from '../api/ops'
import { useHomeExperience } from '../experience/layout'
import { ErrorState, ListSkeleton, Section } from '../ui'
import { costText, formatTokens } from './runModel'
import './runs.css'

type AgentUsage = OpOutput<'agent_usage_list'>

const agentUsageKey = ['runs', 'agent-usage'] as const

/**
 * A1.15: tokens and cost per agent in this workspace over the last 30 days,
 * from runs in projects you can open. A cost split from a usage window a
 * host reported is marked ≈; a window no run was going in is listed apart.
 */
export function AgentUsageSection() {
  const shown = useHomeExperience().shows('tech.run_usage')
  const usage = useQuery({
    enabled: shown,
    queryKey: agentUsageKey,
    queryFn: ({ signal }) => callOp('agent_usage_list', {}, { signal }) as Promise<AgentUsage>,
    refetchInterval: 5 * 60_000,
  })
  if (!shown) return null
  return (
    <Section
      description="Tokens and cost of each agent’s runs over the last 30 days, in projects you can open."
      id="agent-usage"
      title="Usage by agent"
    >
      {usage.isPending ? (
        <ListSkeleton label="Loading usage by agent" rows={3} />
      ) : usage.isError ? (
        <ErrorState error={usage.error} onRetry={() => void usage.refetch()} title="Usage by agent didn’t load" />
      ) : (
        <AgentUsageTable usage={usage.data} />
      )}
    </Section>
  )
}

export function AgentUsageTable({ usage }: { usage: AgentUsage }) {
  if (usage.accounts.length === 0 && usage.notTiedToARun.length === 0) {
    return <p className="aw-runs__quiet">No runs reported any tokens in the last {usage.days} days.</p>
  }
  const estimated = usage.accounts.some((line) => line.confidence === 'estimated')
  return (
    <div className="aw-runs__tablewrap">
      <table className="aw-runs__table">
        <caption className="aw-sr-only">Tokens and cost per agent</caption>
        <thead>
          <tr>
            <th scope="col">Agent</th>
            <th scope="col">Runs</th>
            <th scope="col">Input</th>
            <th scope="col">Output</th>
            <th scope="col">Cost</th>
          </tr>
        </thead>
        <tbody>
          {usage.accounts.map((line) => {
            const cost = costText(line.cost, line.currency, line.confidence === 'estimated' ? 'estimated' : 'exact')
            return (
              <tr key={line.accountId}>
                <th scope="row">{line.name ?? (line.handle ? `@${line.handle}` : 'An agent')}</th>
                <td>{line.runs.toLocaleString()}</td>
                <td>{formatTokens(line.inputTokens)}</td>
                <td>{formatTokens(line.outputTokens)}</td>
                <td title={cost?.hint ?? undefined}>{cost?.text ?? 'Not known'}</td>
              </tr>
            )
          })}
          {usage.notTiedToARun.map((line) => {
            const cost = costText(line.cost, line.currency, 'exact')
            return (
              <tr className="aw-runs__untied" key={`untied-${line.accountId}`}>
                <th scope="row">
                  {line.name ?? 'An agent'} <span className="aw-runs__quiet">· not tied to a run</span>
                </th>
                <td>—</td>
                <td>{formatTokens(line.inputTokens)}</td>
                <td>{formatTokens(line.outputTokens)}</td>
                <td>{cost?.text ?? 'Not known'}</td>
              </tr>
            )
          })}
        </tbody>
      </table>
      {estimated ? (
        <p className="aw-runs__quiet">≈ marks usage a host reported for a stretch of time, shared out across the runs going then.</p>
      ) : null}
      {usage.notTiedToARun.length > 0 ? (
        <p className="aw-runs__quiet">“Not tied to a run” is usage reported for a time when none of the agent’s runs was going.</p>
      ) : null}
    </div>
  )
}
