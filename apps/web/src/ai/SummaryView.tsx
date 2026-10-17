import type { Summary } from './api'

/** A short catch-up: a few lines, and the outcome or status when there is one. */
export function SummaryView({ summary }: { summary: Summary }) {
  return (
    <div className="aw-ai-summary">
      <ul className="aw-ai-summary__lines">
        {summary.lines.map((line, index) => (
          <li key={index}>{line}</li>
        ))}
      </ul>
      {summary.outcome ? (
        <p className="aw-ai-summary__extra">
          <strong>Outcome:</strong> {summary.outcome}
        </p>
      ) : null}
      {summary.status ? (
        <p className="aw-ai-summary__extra">
          <strong>Where it stands:</strong> {summary.status}
        </p>
      ) : null}
      {summary.model ? (
        <p className="aw-ai-summary__by">
          Summarized by {summary.model} from {summary.entries} {summary.entries === 1 ? 'entry' : 'entries'}. Check anything important against the
          source.
        </p>
      ) : null}
    </div>
  )
}
