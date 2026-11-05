import { matchRoutes } from 'react-router-dom'

// Modules (AWD-75): which module each entry point belongs to, read from the
// manifests in `modules/` at the root of the repository, the same files the
// server reads. Only each manifest's `slots` and `name` are bundled here;
// the rest of a manifest stays on the server, which also says which modules
// are off for the person (`experience.moduleOn`). `slots.test.ts` fails on a
// route, palette entry, shortcut, slash command, settings page, Home section
// or sidebar row that no manifest claims.

export type ModuleSlots = {
  pages: string[]
  parts: string[]
  sidebar: string[]
  home: string[]
  settings: string[]
  palette: string[]
  shortcuts: string[]
  slash: string[]
  setup: string[]
}

export type SlotKind = keyof ModuleSlots

const SLOTS = import.meta.glob<ModuleSlots>(['../../../../modules/*.json', '!../../../../modules/presets.json'], {
  eager: true,
  import: 'slots',
})
const NAMES = import.meta.glob<string>(['../../../../modules/*.json', '!../../../../modules/presets.json'], {
  eager: true,
  import: 'name',
})

const idOf = (path: string) => path.slice(path.lastIndexOf('/') + 1).replace(/\.json$/, '')

/** Every core module's slots, by module id. */
export const MODULE_SLOTS: Record<string, ModuleSlots> = Object.fromEntries(
  Object.entries(SLOTS).map(([path, slots]) => [idOf(path), slots]),
)

const MODULE_NAMES: Record<string, string> = Object.fromEntries(
  Object.entries(NAMES).map(([path, name]) => [idOf(path), name]),
)

/** A module's name, such as "Tasks". */
export function moduleName(module: string) {
  return MODULE_NAMES[module] ?? module
}

/** How well a pattern (exact, or ending in `*`) matches a value; null when it doesn't. */
export function patternScore(pattern: string, value: string): number | null {
  if (pattern.endsWith('*')) {
    const prefix = pattern.slice(0, -1)
    return value.startsWith(prefix) ? prefix.length : null
  }
  return pattern === value ? pattern.length + 1 : null
}

/** The module whose `kind` slots claim `value` most specifically, or null. */
export function moduleOf(kind: SlotKind, value: string): string | null {
  let best: { module: string; score: number } | null = null
  for (const [module, slots] of Object.entries(MODULE_SLOTS)) {
    for (const pattern of slots[kind] ?? []) {
      const score = patternScore(pattern, value)
      if (score !== null && (!best || score > best.score)) best = { module, score }
    }
  }
  return best?.module ?? null
}

const PAGE_ROUTES = Object.entries(MODULE_SLOTS).flatMap(([module, slots]) =>
  slots.pages.map((path) => ({ path, module })),
)

/** The module the page at `pathname` belongs to, matched the way the router matches routes. */
export function moduleOfPath(pathname: string): string | null {
  const matches = matchRoutes(PAGE_ROUTES, pathname)
  const route = matches?.[matches.length - 1]?.route as { module?: string } | undefined
  return route?.module ?? null
}
