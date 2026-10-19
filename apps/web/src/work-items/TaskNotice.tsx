import type { ReactNode } from 'react'

import { Icon, type IconGlyph } from '../ui'

export type NoticeTone = 'muted' | 'agent' | 'warning' | 'danger'

/** A short note under a task's title: who's on it, what's in the way, a save to settle. */
export function TaskNotice({
  actions,
  children,
  icon,
  live,
  title,
  tone,
}: {
  actions?: ReactNode
  children?: ReactNode
  icon: IconGlyph
  /** Announce it: an alert for a save that needs a decision, a status for news. */
  live?: 'alert' | 'status'
  title: ReactNode
  tone: NoticeTone
}) {
  return (
    <div className={`aw-tasknote aw-tasknote--${tone}`} role={live}>
      <Icon className="aw-tasknote__icon" name={icon} size={16} />
      <div className="aw-tasknote__body">
        <p className="aw-tasknote__title">{title}</p>
        {children ? <div className="aw-tasknote__text">{children}</div> : null}
        {actions ? <div className="aw-tasknote__actions">{actions}</div> : null}
      </div>
    </div>
  )
}
