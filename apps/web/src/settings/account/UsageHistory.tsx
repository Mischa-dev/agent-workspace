import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { Banner, Button, ListSkeleton, countLabel, describeLoadError, formatDateTime, formatFullDate, formatShortDate } from '../../ui'
import { getHistory, usageKeys, type AccountHistory, type WindowHistory } from './usageApi'

/**
 * The usage page's history (U5): each window over the last two weeks, how
 * much each past cycle used and left unused, runs per cycle, and runs (and
 * cost, once runs record it) per project.
 */
export function UsageHistory({ accountId }: { accountId: string }) {
  const [open, setOpen] = useState(false)
  const history = useQuery({ queryKey: usageKeys.history(accountId), queryFn: () => getHistory(accountId), enabled: open })
  return (
    <div className="aw-usage__block">
      <div className="aw-usage__blockhead">
        <h3 className="aw-usage__blocktitle">How much you use</h3>
        <Button aria-expanded={open} onClick={() => setOpen((value) => !value)} size="sm" variant="ghost">
          {open ? 'Hide' : 'Show history'}
        </Button>
      </div>
      {!open ? (
        <p className="aw-usage__at">Each window over time, what went unused before each reset, and runs per project.</p>
      ) : history.isPending ? (
        <ListSkeleton label="Loading usage history" rows={3} />
      ) : history.isError ? (
        <Banner onRetry={() => void history.refetch()} title="The history didn’t load." tone="danger">
          {describeLoadError(history.error)}
        </Banner>
      ) : (
        <HistoryBody history={history.data} />
      )}
    </div>
  )
}

function HistoryBody({ history }: { history: AccountHistory }) {
  if (history.windows.length === 0 && history.projects.length === 0) {
    return <p className="aw-usage__at">No history yet. It builds up as the account’s usage is read and runs finish.</p>
  }
  return (
    <div className="aw-history">
      {history.windows.map((window) => (
        <WindowCycles key={window.key} window={window} />
      ))}
      <ProjectTable history={history} />
    </div>
  )
}

function wasteText(window: WindowHistory) {
  if (window.lastCycleUnusedPercent == null) return null
  const average =
    window.averageUnusedPercent != null && window.cycles.filter((cycle) => !cycle.current).length > 1
      ? `, ${window.averageUnusedPercent}% on average`
      : ''
  return `${window.lastCycleUnusedPercent}% went unused last cycle${average}.`
}

function WindowCycles({ window }: { window: WindowHistory }) {
  const waste = wasteText(window)
  return (
    <div className="aw-history__window">
      <p className="aw-usage__label">
        {window.label}
        {waste ? <span className="aw-usage__at"> · {waste}</span> : null}
      </p>
      <UsageChart label={window.label} points={window.points} />
      {window.cycles.length > 0 ? (
        <div className="aw-history__tablewrap">
          <table className="aw-history__table">
            <caption className="aw-sr-only">{window.label} cycles</caption>
            <thead>
              <tr>
                <th scope="col">Reset</th>
                <th scope="col">Used</th>
                <th scope="col">Unused</th>
                <th scope="col">Runs</th>
              </tr>
            </thead>
            <tbody>
              {window.cycles.map((cycle) => (
                <tr key={cycle.resetsAt}>
                  <td title={formatFullDate(cycle.resetsAt)}>
                    {formatDateTime(cycle.resetsAt)}
                    {cycle.current ? ' (now)' : ''}
                  </td>
                  <td>{cycle.peakUsedPercent != null ? `${cycle.peakUsedPercent}%` : '–'}</td>
                  <td>{cycle.unusedPercent != null ? `${cycle.unusedPercent}%` : '–'}</td>
                  <td>{cycle.runs ?? '–'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : null}
    </div>
  )
}

const WIDTH = 600
const HEIGHT = 120

/** Percent used over time, as a line from 0 to 100%. */
function UsageChart({ label, points }: { label: string; points: { at: string; usedPercent: number }[] }) {
  if (points.length < 2) return null
  const times = points.map((point) => Date.parse(point.at))
  const start = Math.min(...times)
  const span = Math.max(1, Math.max(...times) - start)
  const path = points
    .map((point, index) => {
      const x = ((times[index] - start) / span) * WIDTH
      const y = HEIGHT - (Math.max(0, Math.min(100, point.usedPercent)) / 100) * HEIGHT
      return `${index === 0 ? 'M' : 'L'}${x.toFixed(1)},${y.toFixed(1)}`
    })
    .join(' ')
  const last = points[points.length - 1]
  return (
    <figure className="aw-history__chart">
      <svg
        aria-label={`${label} use from ${formatShortDate(points[0].at)} to ${formatShortDate(last.at)}, now ${last.usedPercent}%`}
        preserveAspectRatio="none"
        role="img"
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
      >
        <line className="aw-history__grid" x1="0" x2={WIDTH} y1={HEIGHT / 2} y2={HEIGHT / 2} />
        <path className="aw-history__line" d={path} />
      </svg>
      <figcaption className="aw-usage__at">
        {formatShortDate(points[0].at)} to {formatShortDate(last.at)} · 0 to 100% used
      </figcaption>
    </figure>
  )
}

function ProjectTable({ history }: { history: AccountHistory }) {
  if (history.projects.length === 0) {
    return <p className="aw-usage__at">No runs on this account in the last {countLabel(history.projectDays, 'day')}.</p>
  }
  return (
    <div className="aw-history__tablewrap">
      <table className="aw-history__table">
        <caption className="aw-history__caption">Runs per project, last {countLabel(history.projectDays, 'day')}</caption>
        <thead>
          <tr>
            <th scope="col">Project</th>
            <th scope="col">Runs</th>
            <th scope="col">Cost</th>
          </tr>
        </thead>
        <tbody>
          {history.projects.map((project) => (
            <tr key={project.projectId ?? 'hidden'}>
              <td>{project.projectId ? <Link to={`/projects/${project.projectId}`}>{project.projectName}</Link> : project.projectName}</td>
              <td>{project.runs}</td>
              <td>
                {project.cost != null
                  ? new Intl.NumberFormat(undefined, { style: 'currency', currency: project.currency ?? 'USD' }).format(project.cost)
                  : '–'}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {!history.costTracked ? <p className="aw-usage__at">Cost shows here once runs on this account record what they cost.</p> : null}
    </div>
  )
}
