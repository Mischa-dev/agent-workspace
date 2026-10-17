import { runSettingsFrom } from '../execution/api'
import type { Run } from './threadTypes'

/** "claude_code" → "Claude Code": a harness kind as people read it. */
export function harnessLabel(kind: string) {
  return kind
    .split(/[_\s-]+/)
    .filter(Boolean)
    .map((word) => word.charAt(0).toLocaleUpperCase() + word.slice(1))
    .join(' ')
}

/**
 * Who a run is, from the harness frozen on it: the name to show and a stable
 * avatar seed. Runs from before settings were frozen read as "Agent".
 */
export function runAgent(run: Run | null | undefined) {
  const harness = runSettingsFrom(run?.config)?.harness
  return harness ? { name: harnessLabel(harness), seed: harness } : { name: 'Agent', seed: 'agent' }
}
