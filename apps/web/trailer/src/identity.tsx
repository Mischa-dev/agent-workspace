import { Radar } from 'lucide-react'
import type { ComponentProps, CSSProperties } from 'react'

import { Avatar as AppAvatar, AvatarStack as AppAvatarStack } from '../../src/ui/identity'
import { markFor } from './logos'

/*
 * The trailer's build swaps the app's identity module for this one
 * (vite.config.ts), so every avatar the app's own components draw, in message
 * rows, reply status, board cards, task properties and mention lists, shows
 * the demo agent's own mark. Everything else comes from the app unchanged.
 */

export * from '../../src/ui/identity'

type AvatarProps = ComponentProps<typeof AppAvatar>

export function Avatar(props: AvatarProps) {
  const mark = props.src || props.kind === 'person' ? undefined : markFor(props.seed)
  if (!mark) return <AppAvatar {...props} />
  const kind = props.kind ?? 'agent'
  const className = ['tr-logo', 'src' in mark && mark.light ? 'tr-logo--light' : '', props.className].filter(Boolean).join(' ')
  if ('src' in mark) return <AppAvatar {...props} className={className} kind={kind} src={mark.src} />
  // A drawn mark sits in the app's own avatar box, as an image would.
  const size = props.size ?? 36
  const classes = ['aw-avatar', 'aw-avatar--square', size <= 24 ? 'is-small' : '', size >= 56 ? 'is-large' : '', props.working ? 'is-working' : '', className].filter(Boolean).join(' ')
  const ground = mark.glyph === 'scout' ? 'var(--swatch-blue)' : 'var(--brand-deep)'
  return (
    <span aria-hidden className={classes} style={{ '--av-size': `${size}px`, '--av-bg': ground } as CSSProperties}>
      {mark.glyph === 'scout' ? <Radar color="white" size={size * 0.6} strokeWidth={2.2} /> : <WorkspaceMark size={size * 0.74} />}
    </span>
  )
}

export function AvatarStack({ max = 3, people, size = 24 }: ComponentProps<typeof AppAvatarStack>) {
  return (
    <span aria-hidden="true" className="aw-stack">
      {people.slice(0, max).map((person) => (
        <Avatar key={person.id ?? person.name} kind={person.kind} name={person.name} seed={person.id} size={size} />
      ))}
    </span>
  )
}

/** The app's mark (public/icon.svg), for the agent that comes with the workspace. */
function WorkspaceMark({ size }: { size: number }) {
  return (
    <svg height={size} viewBox="0 0 512 512" width={size}>
      <circle cx="193" cy="193" fill="var(--brand)" r="87" />
      <rect fill="var(--brand-deep)" height="219" rx="71.2" width="219" x="209.5" y="209.5" />
      <rect fill="var(--agent-working)" height="174" rx="48.7" width="174" x="232" y="232" />
    </svg>
  )
}
