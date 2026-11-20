import { Link } from 'react-router-dom'

import { projectLocationHref } from '../projects/locations'
import { StatusChip, formatRelative } from '../ui'
import { healthTone, type LatestUpdate } from './projectStatusModel'
import './projectStatus.css'

/** A project's latest update in one line: its health, its name and who posted it when. */
export function UpdateLine({ showHealth = true, spaceId, update }: { showHealth?: boolean; spaceId: string; update: LatestUpdate }) {
  return (
    <span className="aw-project-update">
      {showHealth && update.health ? <StatusChip tone={healthTone(update.health)}>{update.health}</StatusChip> : null}
      <Link to={projectLocationHref(spaceId, { kind: 'thread', conversationId: update.threadId })}>{update.name}</Link>
      <span className="aw-project-update__meta">
        {update.author ? `${update.author.name}, ` : ''}
        <time dateTime={update.postedAt}>{formatRelative(update.postedAt)}</time>
      </span>
    </span>
  )
}
