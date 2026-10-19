import { humanize } from '../projects/format'

const REASONING: Record<string, string> = { xhigh: 'Extra high' }

/** A reasoning level as people read it: `high` is High, `xhigh` Extra high. */
export function reasoningLabel(level: string) {
  return REASONING[level] ?? humanize(level)
}
