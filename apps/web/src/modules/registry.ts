import type { Experience } from '../experience/layout'
import type {
  CaptureRegistration,
  ComposerExtraRegistration,
  MakerRegistration,
  PaletteEntry,
  ShortcutRegistration,
  SidebarRow,
  ThingRegistration,
  ThingTabRegistration,
  WebModule,
} from './define'
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

/** The G-then-a-letter keys every module brings, in today's order, with each key's module (W1.13). */
export function moduleShortcuts(modules: readonly WebModule[] = WEB_MODULES): (ShortcutRegistration & { module: string })[] {
  return modules
    .flatMap((module) => (module.shortcuts ?? []).map((shortcut) => ({ ...shortcut, module: module.id })))
    .sort((left, right) => left.rank - right.rank)
}

/** The kinds of thing the modules open, with each kind's module (W1.13). */
export function moduleThings(modules: readonly WebModule[] = WEB_MODULES): (ThingRegistration & { module: string })[] {
  return modules.flatMap((module) => (module.things ?? []).map((thing) => ({ ...thing, module: module.id })))
}

/** What the modules let a person capture, where their module is on (W1.13). */
export function captureEntries(
  experience: Pick<Experience, 'moduleOn'>,
  modules: readonly WebModule[] = WEB_MODULES,
): (CaptureRegistration & { module: string })[] {
  return modules
    .filter((module) => experience.moduleOn(module.id))
    .flatMap((module) => (module.capture ?? []).map((entry) => ({ ...entry, module: module.id })))
}

/** What a registration shows where: its module is on and the layout shows its part. */
type Where = Pick<Experience, 'shows' | 'moduleOn'>

function shown<T extends { part?: Parameters<Experience['shows']>[0]; rank: number }>(
  experience: Where,
  modules: readonly WebModule[],
  pick: (module: WebModule) => readonly T[] | undefined,
): (T & { module: string })[] {
  return modules
    .filter((module) => experience.moduleOn(module.id))
    .flatMap((module) => (pick(module) ?? []).filter((entry) => !entry.part || experience.shows(entry.part)).map((entry) => ({ ...entry, module: module.id })))
    .sort((left, right) => left.rank - right.rank)
}

/** The tabs modules add to a thing of this kind's detail view, in order (M1.5). */
export function thingTabs(thing: string, experience: Where, modules: readonly WebModule[] = WEB_MODULES): (ThingTabRegistration & { module: string })[] {
  return shown(experience, modules, (module) => module.tabs).filter((tab) => tab.thing === thing)
}

/** What modules add to the message composer here, in order (M1.5). */
export function composerExtras(experience: Where, modules: readonly WebModule[] = WEB_MODULES): (ComposerExtraRegistration & { module: string })[] {
  return shown(experience, modules, (module) => module.composer)
}

/** What modules add to a space's make menu here, in order (M1.5). */
export function makers(experience: Where, modules: readonly WebModule[] = WEB_MODULES): (MakerRegistration & { module: string })[] {
  return shown(experience, modules, (module) => module.makers)
}
