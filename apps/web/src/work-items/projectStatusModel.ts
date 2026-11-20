import type { OpOutput } from '../api/ops'
import type { ChipTone } from '../ui'

export type ProjectStatus = OpOutput<'project_status_get'>
export type LatestUpdate = NonNullable<ProjectStatus['latestUpdate']>

/** The healths a project can have, as the server stores and shows them. */
export const HEALTHS = [
  { value: 'on_track', label: 'On track' },
  { value: 'at_risk', label: 'At risk' },
  { value: 'off_track', label: 'Off track' },
] as const

export type Health = (typeof HEALTHS)[number]['value']

/** How a health reads as a chip, by its key or its label. */
export function healthTone(health: string | null | undefined): ChipTone {
  const word = (health ?? '').toLowerCase().replace(/[_-]/g, ' ')
  if (word === 'on track') return 'success'
  if (word === 'at risk') return 'warning'
  if (word === 'off track') return 'danger'
  return 'muted'
}
