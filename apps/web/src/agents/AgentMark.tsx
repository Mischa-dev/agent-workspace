import type { CSSProperties } from 'react'

import { initials } from '../ui'
import { profileSwatch } from './presenceModel'

/** An agent's own mark: its avatar text on its color, square like every agent's. */
export function AgentMark({ avatar, color, name, seed, size = 32 }: { avatar: string | null; color: string | null; name: string; seed: string; size?: number }) {
  return (
    <span
      aria-hidden
      className={['aw-avatar', 'aw-avatar--square', size <= 24 ? 'is-small' : ''].filter(Boolean).join(' ')}
      style={{ '--av-size': `${size}px`, '--av-bg': `var(--swatch-${profileSwatch(color, seed)})` } as CSSProperties}
    >
      <span className="aw-avatar__initials">{avatar?.trim() || initials(name)}</span>
    </span>
  )
}
