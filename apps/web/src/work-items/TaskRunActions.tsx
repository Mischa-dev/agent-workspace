import { Icon, Tooltip } from '../ui'
import { buildOpenInCodexLink } from './codexDeepLink'
import type { WorkItem } from './types'

/** The app a deep link opens, named from its scheme ("codex://…" opens Codex). */
function appName(href: string) {
  const scheme = href.slice(0, href.indexOf(':'))
  return scheme ? scheme.charAt(0).toUpperCase() + scheme.slice(1) : ''
}

/**
 * Opens a coding app on this computer with the task and its one linked
 * repository ready. Shown only when that can work; otherwise the card would be
 * a dead button on most tasks.
 */
export function OpenInCodex({ item, projectArchived }: { item: WorkItem; projectArchived: boolean }) {
  if (projectArchived) return null
  const result = buildOpenInCodexLink(item, window.location.origin)
  if (!result.href) return null
  const app = appName(result.href)
  return (
    <section aria-labelledby={`open-locally-heading-${item.id}`} className="aw-sidecard">
      <h2 className="aw-sidecard__title" id={`open-locally-heading-${item.id}`}>
        Work on it locally
      </h2>
      <p className="aw-sidecard__text">Opens {app || 'the coding app'} on your computer with this task and its repository ready. Nothing starts until you send it.</p>
      <Tooltip label={app ? `Opens in ${app}` : 'Opens on your computer'}>
        <a className="aw-btn aw-btn--secondary aw-btn--md" href={result.href}>
          <Icon name="terminal" />
          <span className="aw-btn__label">Open locally</span>
        </a>
      </Tooltip>
    </section>
  )
}
