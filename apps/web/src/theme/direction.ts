// Hill climb round 1 (AWD-85): three whole-product visual directions behind a
// switch, so each can be seen in the real app. `?direction=workroom`,
// `ledger` or `paper` picks one and remembers it; `?direction=current` goes
// back to today's look. Draft only: Mischa picks one, then the tokens move to
// match it and this switch goes away.

export const DIRECTIONS = ['workroom', 'ledger', 'paper'] as const
export type Direction = (typeof DIRECTIONS)[number]

const STORAGE_KEY = 'agent-workspace:direction'

function isDirection(value: string | null): value is Direction {
  return DIRECTIONS.includes(value as Direction)
}

/** Reads the direction from the address or storage and sets it on <html>. */
export function applyDirection(root: HTMLElement = document.documentElement) {
  let stored: string | null = null
  try {
    const asked = new URLSearchParams(window.location.search).get('direction')
    if (asked === 'current') window.localStorage.removeItem(STORAGE_KEY)
    else if (isDirection(asked)) window.localStorage.setItem(STORAGE_KEY, asked)
    stored = window.localStorage.getItem(STORAGE_KEY)
  } catch {
    // Storage can be off (private mode); the address still works for one load.
  }
  if (isDirection(stored)) root.dataset.direction = stored
  else delete root.dataset.direction
}
