import home from 'virtual:modules/slots/home'
import pages from 'virtual:modules/slots/pages'
import palette from 'virtual:modules/slots/palette'
import parts from 'virtual:modules/slots/parts'
import settings from 'virtual:modules/slots/settings'
import setup from 'virtual:modules/slots/setup'
import shortcuts from 'virtual:modules/slots/shortcuts'
import sidebar from 'virtual:modules/slots/sidebar'
import slash from 'virtual:modules/slots/slash'

import { MODULE_IDS } from './names'
import { ownerOf, type Claims } from './ownership'

// Modules (AWD-75): which module each entry point belongs to, read from the
// manifests in `modules/` at the root of the repository, the same files the
// server reads. The build projects each kind of claim on its own
// (`virtual:modules/slots/<kind>`, see vite.config.ts), and the app imports
// only the kinds it asks about (ownership.ts, pageOwners.ts, names.ts). This
// is every kind at once, for `slots.test.ts`, which fails on a route, palette
// entry, shortcut, slash command, settings page, Home section or sidebar row
// that no manifest claims. Nothing the app loads imports it.

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

export const CLAIMS: Record<SlotKind, Claims> = { pages, parts, sidebar, home, settings, palette, shortcuts, slash, setup }

/** Every core module's slots, by module id. */
export const MODULE_SLOTS: Record<string, ModuleSlots> = Object.fromEntries(
  MODULE_IDS.map((module) => [
    module,
    Object.fromEntries(
      (Object.keys(CLAIMS) as SlotKind[]).map((kind) => [
        kind,
        CLAIMS[kind].filter(([, owner]) => owner === module).map(([pattern]) => pattern),
      ]),
    ) as ModuleSlots,
  ]),
)

/** The module whose `kind` slots claim `value` most specifically, or null. */
export function moduleOf(kind: SlotKind, value: string): string | null {
  return ownerOf(CLAIMS[kind], value)
}

export { moduleName } from './names'
export { moduleSwitchable, patternScore } from './ownership'
export { moduleOfPath } from './pageOwners'
