import { SWATCHES, swatchFor, type Swatch } from '../ui'
import type { AgentPresence } from './api'

/** What the agent is doing, in a few words: "Working on KEY-12", "Waiting on you" or "Idle". */
export function presenceLabel(agent: Pick<AgentPresence, 'state' | 'taskKey' | 'taskTitle' | 'threadName' | 'active'>) {
  if (agent.state === 'waiting') return 'Waiting on you'
  if (agent.state === 'working') {
    const what = agent.taskKey ?? agent.taskTitle ?? agent.threadName
    const more = agent.active > 1 ? ` and ${agent.active - 1} more` : ''
    return what ? `Working on ${what}${more}` : 'Working'
  }
  return 'Idle'
}

const ALIASES: Record<string, Swatch> = { violet: 'orchid', purple: 'orchid', amber: 'rust', orange: 'rust', red: 'rose', pink: 'rose', gray: 'slate', grey: 'slate' }

/** The kit swatch for a profile's color word, so any word the server accepts still gets a color. */
export function profileSwatch(color: string | null | undefined, seed: string): Swatch {
  if (!color) return swatchFor(seed)
  if ((SWATCHES as readonly string[]).includes(color)) return color as Swatch
  return ALIASES[color] ?? swatchFor(color)
}

/** Allowed tools typed one per line or separated by commas, without blanks or repeats. */
export function parseTools(text: string) {
  const seen = new Set<string>()
  for (const part of text.split(/[\n,]/)) {
    const tool = part.trim()
    if (tool) seen.add(tool)
  }
  return [...seen]
}
