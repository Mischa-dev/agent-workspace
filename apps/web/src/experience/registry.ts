import data from './registry.json'
import type { Word } from './layout'
import type { NounId, PartId } from './parts'

// The layout registry (AWD-66), typed, and what a layout shows worked out the
// way the server does, for the Layout settings pages' previews. Import it only
// from lazy pages: the registry is too big for the initial bundle.

export type RegistryGroup = { id: string; label: string; description: string }
export type RegistryPart = { id: PartId; label: string; description: string; group: string; default: boolean }
export type RegistryNoun = { id: NounId; label: string; one: string; many: string }
export type BuiltinPreset = {
  id: string
  name: string
  description: string
  tone: string
  hide?: string[]
  only?: string[]
  lead?: string[]
  words?: Partial<Record<NounId, Word>>
}

type Registry = { groups: RegistryGroup[]; parts: RegistryPart[]; nouns: RegistryNoun[]; presets: BuiltinPreset[] }

export const REGISTRY = data as unknown as Registry

/** A project's or the workspace's own layout: a preset, with switches and words changed from it. */
export type Layout = {
  preset: string
  changes?: Partial<Record<PartId, boolean>>
  words?: Partial<Record<NounId, Word>>
}

/** A preset the workspace saved: a built-in one with its own changes and words. */
export type SavedPreset = {
  id: string
  name: string
  description: string
  base: string
  changes: Partial<Record<PartId, boolean>>
  words: Partial<Record<NounId, Word>>
  version: number
  removed: boolean
}

/** What a layout shows: the parts, the words and what goes first. */
export type Shape = { visible: Set<PartId>; words: Partial<Record<NounId, Word>>; lead: string[] }

export const builtinPreset = (id: string) => REGISTRY.presets.find((preset) => preset.id === id) ?? null

export function everything(): Shape {
  return { visible: new Set(REGISTRY.parts.filter((part) => part.default).map((part) => part.id)), words: {}, lead: [] }
}

function builtinShape(preset: BuiltinPreset): Shape {
  const visible = REGISTRY.parts
    .filter((part) => (preset.only ? preset.only.includes(part.id) : part.default && !preset.hide?.includes(part.id)))
    .map((part) => part.id)
  return { visible: new Set(visible), words: { ...preset.words }, lead: [...(preset.lead ?? [])] }
}

function apply(shape: Shape, changes: Layout['changes'] = {}, words: Layout['words'] = {}): Shape {
  const visible = new Set(shape.visible)
  for (const part of REGISTRY.parts) {
    const change = changes[part.id]
    if (change === true) visible.add(part.id)
    if (change === false) visible.delete(part.id)
  }
  return { visible, words: { ...shape.words, ...words }, lead: shape.lead }
}

/** What a preset (a built-in one's id or a saved one's ID) shows; null when there's no such preset. */
export function presetShape(reference: string, saved: SavedPreset[]): Shape | null {
  const preset = builtinPreset(reference)
  if (preset) return builtinShape(preset)
  const own = saved.find((candidate) => candidate.id === reference)
  if (!own) return null
  const base = builtinPreset(own.base)
  return apply(base ? builtinShape(base) : everything(), own.changes, own.words)
}

/** What a layout shows; nothing chosen shows everything. */
export function layoutShape(layout: Layout | null, saved: SavedPreset[]): Shape {
  if (!layout) return everything()
  return apply(presetShape(layout.preset, saved) ?? everything(), layout.changes, layout.words)
}

/** A word as a shape says it. */
export function shapeWord(shape: Shape, noun: NounId, form: 'one' | 'many' = 'one') {
  return (shape.words[noun] ?? REGISTRY.nouns.find((entry) => entry.id === noun) ?? { one: noun, many: `${noun}s` })[form]
}

/**
 * The smallest layout that shows `visible` with `words`, starting from
 * `preset`: only the switches and words that differ from the preset's.
 */
export function layoutFrom(preset: string, visible: Set<PartId>, words: Partial<Record<NounId, Word>>, saved: SavedPreset[]): Layout {
  const base = presetShape(preset, saved) ?? everything()
  const changes: Partial<Record<PartId, boolean>> = {}
  for (const part of REGISTRY.parts) {
    if (visible.has(part.id) !== base.visible.has(part.id)) changes[part.id] = visible.has(part.id)
  }
  const renamed: Partial<Record<NounId, Word>> = {}
  for (const noun of REGISTRY.nouns) {
    const word = words[noun.id]
    const own = base.words[noun.id] ?? { one: noun.one, many: noun.many }
    if (word && word.one.trim() && word.many.trim() && (word.one.trim() !== own.one || word.many.trim() !== own.many)) {
      renamed[noun.id] = { one: word.one.trim(), many: word.many.trim() }
    }
  }
  return {
    preset,
    ...(Object.keys(changes).length ? { changes } : {}),
    ...(Object.keys(renamed).length ? { words: renamed } : {}),
  }
}

/** A preset's name, for a reference that may be built in, saved or gone. */
export function presetName(reference: string, saved: SavedPreset[]) {
  return builtinPreset(reference)?.name ?? saved.find((preset) => preset.id === reference)?.name ?? 'A removed preset'
}
