// W1.40: the visual directions the owner picks from (styles/directions.css).
// None ships as a choice. `?direction=<id>` previews one for the rest of the
// tab's session and `?direction=none` ends it; the stylesheet loads only then.

export const DIRECTIONS = ['quiet', 'paper', 'frame'] as const
export type Direction = (typeof DIRECTIONS)[number]

const KEY = 'agent-workspace:direction-preview'

const isDirection = (value: string | null): value is Direction => DIRECTIONS.some((direction) => direction === value)

/** The direction this tab previews, from the address or what it chose earlier. */
export function previewedDirection(search = window.location.search): Direction | null {
  const asked = new URLSearchParams(search).get('direction')
  try {
    if (asked === 'none') window.sessionStorage.removeItem(KEY)
    else if (isDirection(asked)) window.sessionStorage.setItem(KEY, asked)
    const kept = window.sessionStorage.getItem(KEY)
    return isDirection(kept) ? kept : null
  } catch {
    return isDirection(asked) ? asked : null
  }
}

/** Loads the directions and marks <html> with the previewed one, if any. */
export function applyDirectionPreview() {
  const direction = previewedDirection()
  if (!direction) return
  void import('../styles/directions.css').then(() => {
    document.documentElement.dataset.direction = direction
  })
}
