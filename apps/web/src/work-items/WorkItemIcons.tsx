import { priorityLabel } from './constants'
import type { WorkItemPriority } from './types'

/** How far along each workflow category is, drawn as a filling circle. */
const STATUS_FILL: Record<string, number> = {
  backlog: 0,
  unstarted: 0,
  started: 0.5,
  review: 0.75,
  verification: 0.75,
}

const PIE = 2 * Math.PI * 2.75

/** A status's category as a small circle: dashed for backlog, filling as work moves, checked when done. */
export function StatusIcon({ category, label }: { category: string; label?: string }) {
  const known = category in STATUS_FILL || category === 'completed' || category === 'canceled'
  const tone = known ? category : 'unstarted'
  return (
    <svg
      aria-hidden={label ? undefined : true}
      aria-label={label}
      className={`aw-status aw-status--${tone}`}
      focusable="false"
      height="14"
      role={label ? 'img' : undefined}
      viewBox="0 0 14 14"
      width="14"
    >
      {tone === 'completed' || tone === 'canceled' ? (
        <>
          <circle cx="7" cy="7" fill="currentColor" r="6.25" />
          {tone === 'completed' ? (
            <path className="aw-status__mark" d="M4.4 7.2l1.8 1.8 3.5-3.7" fill="none" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.6" />
          ) : (
            <path className="aw-status__mark" d="M5 5l4 4M9 5l-4 4" fill="none" strokeLinecap="round" strokeWidth="1.6" />
          )}
        </>
      ) : (
        <>
          <circle
            cx="7"
            cy="7"
            fill="none"
            r="5.75"
            stroke="currentColor"
            strokeDasharray={tone === 'backlog' ? '2.2 1.8' : undefined}
            strokeWidth="1.5"
          />
          {STATUS_FILL[tone] > 0 ? (
            <circle
              cx="7"
              cy="7"
              fill="none"
              r="2.75"
              stroke="currentColor"
              strokeDasharray={`${STATUS_FILL[tone] * PIE} ${PIE}`}
              strokeWidth="5.5"
              transform="rotate(-90 7 7)"
            />
          ) : null}
        </>
      )}
    </svg>
  )
}

const BARS: Record<WorkItemPriority, number> = { no_priority: 0, low: 1, medium: 2, high: 3, urgent: 3 }

/** Priority as signal bars, or a red square for urgent. */
export function PriorityIcon({ decorative = false, priority }: { decorative?: boolean; priority: WorkItemPriority }) {
  const label = priority === 'no_priority' ? 'No priority' : `${priorityLabel(priority)} priority`
  return (
    <svg
      aria-hidden={decorative || undefined}
      aria-label={decorative ? undefined : label}
      className={`aw-priority aw-priority--${priority}`}
      focusable="false"
      height="14"
      role={decorative ? undefined : 'img'}
      viewBox="0 0 14 14"
      width="14"
    >
      {priority === 'urgent' ? (
        <>
          <rect fill="currentColor" height="12" rx="3" width="12" x="1" y="1" />
          <rect className="aw-priority__mark" height="4.5" rx="0.8" width="1.8" x="6.1" y="3.4" />
          <rect className="aw-priority__mark" height="1.8" rx="0.9" width="1.8" x="6.1" y="9.1" />
        </>
      ) : priority === 'no_priority' ? (
        [1.5, 5.75, 10].map((x) => <rect fill="currentColor" height="1.5" key={x} rx="0.75" width="2.5" x={x} y="6.25" />)
      ) : (
        [4, 7, 10].map((height, index) => (
          <rect
            className={index < BARS[priority] ? undefined : 'aw-priority__off'}
            fill="currentColor"
            height={height}
            key={height}
            rx="0.75"
            width="2.5"
            x={1.5 + index * 4.25}
            y={12.5 - height}
          />
        ))
      )}
    </svg>
  )
}
