import type { CSSProperties } from 'react'

import { initials } from '../ui'
import { profileSwatch } from './presenceModel'

/** At most two characters as people see them, so "🧑‍💻" or "ab" fits the square and "Reviewer" doesn't spill out. */
function markText(avatar: string | null, name: string) {
  const text = avatar?.trim()
  if (!text) return initials(name)
  const characters =
    typeof Intl.Segmenter === 'function'
      ? Array.from(new Intl.Segmenter(undefined, { granularity: 'grapheme' }).segment(text), (part) => part.segment)
      : Array.from(text)
  return characters.slice(0, 2).join('')
}

/** An agent's own mark: its avatar text on its color, square like every agent's. */
export function AgentMark({ avatar, color, name, seed, size = 32 }: { avatar: string | null; color: string | null; name: string; seed: string; size?: number }) {
  return (
    <span
      aria-hidden
      className={['aw-avatar', 'aw-avatar--square', 'aw-agentmark', size <= 24 ? 'is-small' : ''].filter(Boolean).join(' ')}
      style={{ '--av-size': `${size}px`, '--av-bg': `var(--swatch-${profileSwatch(color, seed)})` } as CSSProperties}
    >
      <span className="aw-avatar__initials">{markText(avatar, name)}</span>
    </span>
  )
}
