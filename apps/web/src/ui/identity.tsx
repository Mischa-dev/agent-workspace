import type { CSSProperties, ReactNode } from 'react'

import { Icon, type IconName } from './icons'
import { initials, swatchFor, type ActorKind } from './identityModel'


const TAGS: Record<ActorKind, string | null> = {
  person: null,
  agent: 'Agent',
  worker: 'Worker',
  app: 'App',
}

export function Avatar({
  className,
  kind = 'person',
  label,
  name,
  presence,
  seed,
  size = 36,
  src,
  working = false,
}: {
  className?: string
  kind?: ActorKind
  /** Expose the avatar to assistive technology with this name. */
  label?: boolean
  name: string
  presence?: 'online' | 'away'
  /** Id used for the swatch, so renames keep the color. */
  seed?: string
  size?: number
  src?: string | null
  working?: boolean
}) {
  const square = kind !== 'person'
  const classes = [
    'aw-avatar',
    square ? 'aw-avatar--square' : 'aw-avatar--round',
    size <= 24 ? 'is-small' : '',
    size >= 56 ? 'is-large' : '',
    working ? 'is-working' : '',
    className ?? '',
  ].filter(Boolean).join(' ')
  const described = [name, presence, working ? 'working' : ''].filter(Boolean).join(', ')
  return (
    <span
      aria-hidden={label ? undefined : true}
      aria-label={label ? described : undefined}
      className={classes}
      role={label ? 'img' : undefined}
      style={{
        '--av-size': `${size}px`,
        // Workers (Codex runs) are always slate, so they read as the same tool everywhere.
        '--av-bg': `var(--swatch-${kind === 'worker' ? 'slate' : swatchFor(seed ?? name)})`,
      } as CSSProperties}
    >
      {src ? <img alt="" src={src} /> : <span className="aw-avatar__initials">{initials(name)}</span>}
      {presence && !square ? <span className={`aw-presence aw-presence--${presence}`} /> : null}
    </span>
  )
}

export function AvatarStack({
  max = 3,
  people,
  size = 24,
}: {
  max?: number
  people: { id?: string; name: string; kind?: ActorKind }[]
  size?: number
}) {
  return (
    <span aria-hidden="true" className="aw-stack">
      {people.slice(0, max).map((person) => (
        <Avatar key={person.id ?? person.name} kind={person.kind} name={person.name} seed={person.id} size={size} />
      ))}
    </span>
  )
}

export function ActorTag({ kind }: { kind: ActorKind }) {
  const tag = TAGS[kind]
  return tag ? <span className={`aw-tag aw-tag--${kind}`}>{tag}</span> : null
}

export function Badge({
  className,
  count,
  dot = false,
  label,
  max = 99,
  tone = 'mention',
}: {
  className?: string
  count?: number
  dot?: boolean
  label?: string
  max?: number
  tone?: 'mention' | 'neutral'
}) {
  const classes = ['aw-badge', `aw-badge--${tone}`, dot ? 'aw-badge--dot' : '', className ?? ''].filter(Boolean).join(' ')
  if (dot) return <span aria-label={label} className={classes} role={label ? 'img' : undefined} />
  if (!count) return null
  return (
    <span aria-label={label} className={classes}>
      {count > max ? `${max}+` : count}
    </span>
  )
}

/** Three dots in the agent-working color: something is being written or run right now. */
export function Dots({ label }: { label?: string }) {
  return (
    <span aria-hidden={label ? undefined : true} aria-label={label} className="aw-dots" role={label ? 'img' : undefined}>
      <i />
      <i />
      <i />
    </span>
  )
}

export type ChipTone = 'muted' | 'agent' | 'warning' | 'success' | 'danger'

export function StatusChip({
  children,
  dots = false,
  icon,
  spin = false,
  tone = 'muted',
}: {
  children: ReactNode
  dots?: boolean
  icon?: IconName
  spin?: boolean
  tone?: ChipTone
}) {
  return (
    <span className={`aw-chip aw-chip--${tone}`}>
      {dots ? <Dots /> : icon ? <Icon className={spin ? 'aw-spin' : undefined} name={icon} size={12} /> : null}
      {children}
    </span>
  )
}
