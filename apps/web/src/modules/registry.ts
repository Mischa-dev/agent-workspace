import type { Experience } from '../experience/layout'
import type { PaletteEntry, SidebarRow, WebModule } from './define'
import { MODULE_IDS } from './names'

// M1.4: every core module's web registration, found by the build
// (`../<folder>/module.ts`) and put in manifest order. Each `module.ts` has
// only type imports, so loading them all is cheap.

const found = import.meta.glob<{ default: WebModule }>('../*/module.ts', { eager: true })

/** The folder each registration came from, by module id: what the registry test checks. */
export const REGISTERED_FOLDERS: Readonly<Record<string, string>> = Object.fromEntries(
  Object.entries(found).map(([path, module]) => [module.default.id, path.split('/')[1] ?? path]),
)

const order = (id: string) => {
  const index = MODULE_IDS.indexOf(id)
  return index < 0 ? Number.MAX_SAFE_INTEGER : index
}

/** Every registered module, in manifest order. */
export const WEB_MODULES: readonly WebModule[] = Object.values(found)
  .map((module) => module.default)
  .sort((left, right) => order(left.id) - order(right.id))

/** The registration for a module, if it has one. */
export function webModule(id: string): WebModule | undefined {
  return WEB_MODULES.find((module) => module.id === id)
}

/** A row's words in this layout. */
export function rowLabel(row: SidebarRow, experience: Pick<Experience, 'label'>): string {
  return typeof row.label === 'string' ? row.label : experience.label(row.label.noun, 'many')
}

/** The sidebar rows of `space` that show here, in today's order, with each row's module. */
export function sidebarRows(
  space: SidebarRow['space'],
  experience: Pick<Experience, 'shows' | 'moduleOn'>,
  modules: readonly WebModule[] = WEB_MODULES,
): (SidebarRow & { module: string })[] {
  return modules
    .filter((module) => experience.moduleOn(module.id))
    .flatMap((module) =>
      (module.sidebar ?? [])
        .filter((row) => row.space === space)
        .filter((row) => !row.part || experience.shows(row.part))
        .filter((row) => !row.unless || !experience.shows(row.unless))
        .map((row) => ({ ...row, module: module.id })),
    )
    .sort((left, right) => left.rank - right.rank)
}

/** The palette's entries for `space`, in today's order, with each entry's module. */
export function paletteEntries(
  space: PaletteEntry['space'],
  modules: readonly WebModule[] = WEB_MODULES,
): (PaletteEntry & { module: string })[] {
  return modules
    .flatMap((module) =>
      (module.palette ?? []).filter((entry) => entry.space === space).map((entry) => ({ ...entry, module: module.id })),
    )
    .sort((left, right) => left.rank - right.rank)
}
