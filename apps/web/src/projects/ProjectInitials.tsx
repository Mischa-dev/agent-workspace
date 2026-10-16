import type { CSSProperties } from 'react'

import { swatchFor } from '../ui'
import type { Project } from './types'

/** A Project's key on its color: the same tile the rail shows. */
export function ProjectInitials({ project, size = 28 }: { project: Project; size?: number }) {
  return (
    <span
      aria-hidden="true"
      className="aw-space-mark"
      style={{ '--tile-bg': `var(--swatch-${swatchFor(project.id)})`, '--mark-size': `${size}px` } as CSSProperties}
    >
      {project.key.slice(0, 2)}
    </span>
  )
}
