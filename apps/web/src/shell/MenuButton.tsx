import { useRef } from 'react'

import { Icon } from '../ui'
import { useShellHeader } from './mobileNavigation'

/**
 * The phone menu button: opens the drawer with every space and the current
 * space's channels. `label` names the current space beside the icon. Hidden on
 * wide screens, where the rail and sidebar are always showing.
 */
export function MenuButton({ label }: { label?: string }) {
  const shell = useShellHeader()
  const ref = useRef<HTMLButtonElement>(null)
  if (!shell) return null
  return (
    <button
      aria-controls="project-rail"
      aria-expanded={shell.navigationOpen}
      aria-label={label ? `${label}: open navigation` : 'Open navigation'}
      className="aw-menubutton"
      onClick={() => shell.openNavigation(ref.current)}
      ref={ref}
      type="button"
    >
      <Icon name="menu" size={20} />
      {label ? (
        <span aria-hidden="true" className="aw-menubutton__space">
          {label}
        </span>
      ) : null}
    </button>
  )
}
