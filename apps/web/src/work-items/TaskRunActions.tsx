import { Button, Icon } from '../ui'
import { buildOpenInCodexLink } from './codexDeepLink'
import type { WorkItem } from './types'

export function OpenInCodex({ item, projectArchived }: { item: WorkItem; projectArchived: boolean }) {
  const result = projectArchived
    ? { href: null, reason: 'Restore this Project before opening a work item in Codex.' }
    : buildOpenInCodexLink(item, window.location.origin)
  const reasonId = `open-in-codex-reason-${item.id}`

  return (
    <section aria-labelledby={`open-in-codex-heading-${item.id}`} className="aw-sidecard">
      <h2 className="aw-sidecard__title" id={`open-in-codex-heading-${item.id}`}>
        Work on it locally
      </h2>
      <p className="aw-sidecard__text">Opens Codex on your computer with this work item ready. Nothing starts until you send it there.</p>
      {result.href ? (
        <a className="aw-btn aw-btn--secondary aw-btn--md" href={result.href}>
          <Icon name="terminal" />
          <span className="aw-btn__label">Open in Codex</span>
        </a>
      ) : (
        <>
          <Button aria-describedby={reasonId} disabled icon="terminal">
            Open in Codex
          </Button>
          <p className="aw-sidecard__note" id={reasonId}>
            {result.reason}
          </p>
        </>
      )}
    </section>
  )
}
