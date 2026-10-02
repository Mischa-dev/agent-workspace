import type { WorkItemLabel } from './types'

/**
 * A work item's labels as small chips, read-only. Only the dot carries the
 * label's color, so the text keeps its contrast in both themes. With `max`,
 * the rest collapse into "+N".
 */
export function WorkItemLabels({ labels, max }: { labels: WorkItemLabel[]; max?: number }) {
  if (labels.length === 0) return null
  const shown = max === undefined ? labels : labels.slice(0, max)
  const hidden = labels.slice(shown.length)
  return (
    <ul aria-label="Labels" className="aw-labels">
      {shown.map((label) => (
        <li className="aw-label" key={label.id}>
          <span aria-hidden className="aw-label__dot" style={{ backgroundColor: label.color }} />
          <span className="aw-label__name">{label.name}</span>
        </li>
      ))}
      {hidden.length > 0 ? (
        <li className="aw-label aw-label--more" title={hidden.map((label) => label.name).join(', ')}>
          +{hidden.length}
        </li>
      ) : null}
    </ul>
  )
}
