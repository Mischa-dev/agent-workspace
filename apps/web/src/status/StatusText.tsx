import './status.css'

import type { AccountStatus } from './api'
import { statusLine } from './statusWords'

/** An account's status next to its name; nothing when it has none. */
export function StatusText({ status }: { status: AccountStatus | null }) {
  const line = statusLine(status)
  if (!line) return null
  return (
    <span className="aw-status-text" title={line}>
      {line}
    </span>
  )
}
