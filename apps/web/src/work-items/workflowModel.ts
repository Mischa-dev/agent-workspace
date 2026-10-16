import type { EstimateUnit, WorkflowUpdate, WorkItemWorkflow } from './types'

/** A status or type being edited. `ref` is its id, or a made-up key until the server gives it one. */
export type DraftEntry = {
  ref: string
  id?: string
  name: string
  isDefault: boolean
  taskCount: number
}

export type DraftStatus = DraftEntry & { semanticCategory: string }

export type WorkflowDraft = {
  statuses: DraftStatus[]
  types: DraftEntry[]
  allowAny: boolean
  /** Allowed changes as `from|to` refs, used when allowAny is off. */
  pairs: string[]
  estimateUnit: EstimateUnit
  /** Retired id → the ref its tasks move to. */
  replacements: Record<string, string>
}

export const pairKey = (from: string, to: string) => `${from}|${to}`

function allPairs(refs: string[]) {
  return refs.flatMap((from) => refs.filter((to) => to !== from).map((to) => pairKey(from, to)))
}

/**
 * The editable copy of a workflow. The server doesn't say "any change", so an
 * empty list or a list with every pair both read as allowing any change.
 */
export function draftFromWorkflow(workflow: WorkItemWorkflow): WorkflowDraft {
  const statuses = [...workflow.statuses]
    .sort((a, b) => a.position - b.position)
    .map((status) => ({ ref: status.id, id: status.id, name: status.name, semanticCategory: status.semanticCategory, isDefault: status.isDefault, taskCount: status.taskCount }))
  const types = [...workflow.types]
    .sort((a, b) => a.position - b.position)
    .map((type) => ({ ref: type.id, id: type.id, name: type.name, isDefault: type.isDefault, taskCount: type.taskCount }))
  const pairs = workflow.transitions.map((transition) => pairKey(transition.fromStatusId, transition.toStatusId))
  const every = allPairs(statuses.map((status) => status.ref))
  const allowAny = pairs.length === 0 || every.every((pair) => pairs.includes(pair))
  return { statuses, types, allowAny, pairs: allowAny ? every : pairs, estimateUnit: workflow.estimateUnit, replacements: {} }
}

let counter = 0
/** A key for a status or type the server hasn't seen yet. */
export function newRef(prefix: 'status' | 'type') {
  counter += 1
  return `new-${prefix}-${Date.now().toString(36)}-${counter}`
}

/** Moves entry `index` one place up (-1) or down (+1). */
export function moveEntry<T>(entries: T[], index: number, step: -1 | 1): T[] {
  const target = index + step
  if (target < 0 || target >= entries.length) return entries
  const next = [...entries]
  ;[next[index], next[target]] = [next[target], next[index]]
  return next
}

/** Makes `ref` the only default. */
export function withDefault<T extends DraftEntry>(entries: T[], ref: string): T[] {
  return entries.map((entry) => ({ ...entry, isDefault: entry.ref === ref }))
}

/** Statuses or types that were in the saved workflow and aren't in the draft. */
export function retired<T extends { id: string; name: string; taskCount: number }>(saved: T[], draft: DraftEntry[]) {
  const kept = new Set(draft.map((entry) => entry.id).filter(Boolean))
  return saved.filter((entry) => !kept.has(entry.id))
}

/** Adds a status, letting it move to and from every other status when changes are limited. */
export function addStatus(draft: WorkflowDraft, status: DraftStatus): WorkflowDraft {
  const others = draft.statuses.map((entry) => entry.ref)
  const pairs = draft.allowAny ? draft.pairs : [...draft.pairs, ...others.flatMap((ref) => [pairKey(ref, status.ref), pairKey(status.ref, ref)])]
  return { ...draft, statuses: [...draft.statuses, status], pairs }
}

export function removeStatus(draft: WorkflowDraft, ref: string): WorkflowDraft {
  const statuses = draft.statuses.filter((entry) => entry.ref !== ref)
  if (!statuses.some((entry) => entry.isDefault) && statuses[0]) statuses[0] = { ...statuses[0], isDefault: true }
  const replacements = Object.fromEntries(Object.entries(draft.replacements).filter(([, to]) => to !== ref))
  return { ...draft, statuses, pairs: draft.pairs.filter((pair) => !pair.split('|').includes(ref)), replacements }
}

export function removeType(draft: WorkflowDraft, ref: string): WorkflowDraft {
  const types = draft.types.filter((entry) => entry.ref !== ref)
  if (!types.some((entry) => entry.isDefault) && types[0]) types[0] = { ...types[0], isDefault: true }
  const replacements = Object.fromEntries(Object.entries(draft.replacements).filter(([, to]) => to !== ref))
  return { ...draft, types, replacements }
}

export function togglePair(draft: WorkflowDraft, from: string, to: string): WorkflowDraft {
  const key = pairKey(from, to)
  const pairs = draft.pairs.includes(key) ? draft.pairs.filter((pair) => pair !== key) : [...draft.pairs, key]
  return { ...draft, pairs }
}

export function setAllowAny(draft: WorkflowDraft, allowAny: boolean): WorkflowDraft {
  return { ...draft, allowAny, pairs: allowAny || draft.pairs.length === 0 ? allPairs(draft.statuses.map((entry) => entry.ref)) : draft.pairs }
}

/** What's wrong with the draft in plain words, or null when it can be saved. */
export function draftProblem(draft: WorkflowDraft, saved: WorkItemWorkflow): string | null {
  if (draft.statuses.length === 0) return 'Keep at least one status.'
  if (draft.types.length === 0) return 'Keep at least one type.'
  for (const [entries, noun] of [
    [draft.statuses, 'status'],
    [draft.types, 'type'],
  ] as const) {
    const names = entries.map((entry) => entry.name.trim().toLowerCase())
    if (names.some((name) => !name)) return `Every ${noun} needs a name.`
    const repeated = names.find((name, index) => names.indexOf(name) !== index)
    if (repeated) return `Two ${noun === 'status' ? 'statuses' : 'types'} are called “${entries[names.indexOf(repeated)].name.trim()}”.`
    if (entries.filter((entry) => entry.isDefault).length !== 1) return `Choose one default ${noun}.`
  }
  const refs = new Set([...draft.statuses, ...draft.types].map((entry) => entry.ref))
  for (const gone of [...retired(saved.statuses, draft.statuses), ...retired(saved.types, draft.types)]) {
    if (gone.taskCount === 0) continue
    const to = draft.replacements[gone.id]
    if (!to || !refs.has(to)) return `Choose where the ${gone.taskCount === 1 ? 'task' : `${gone.taskCount} tasks`} in ${gone.name} go.`
  }
  return null
}

/** The PUT body for the draft. */
export function workflowUpdate(draft: WorkflowDraft, saved: WorkItemWorkflow): WorkflowUpdate {
  const reference = (entry: DraftEntry) => (entry.id ? { id: entry.id } : { key: entry.ref })
  const statusRefs = new Set(draft.statuses.map((entry) => entry.ref))
  const goneWithTasks = [...retired(saved.statuses, draft.statuses), ...retired(saved.types, draft.types)].filter((entry) => entry.taskCount > 0)
  const replacements = Object.fromEntries(goneWithTasks.map((entry) => [entry.id, draft.replacements[entry.id]]).filter(([, to]) => Boolean(to)))
  return {
    expectedVersion: saved.version,
    estimateUnit: draft.estimateUnit,
    statuses: draft.statuses.map((entry) => ({ ...reference(entry), name: entry.name.trim(), semanticCategory: entry.semanticCategory, isDefault: entry.isDefault })),
    types: draft.types.map((entry) => ({ ...reference(entry), name: entry.name.trim(), isDefault: entry.isDefault })),
    transitions: draft.allowAny
      ? 'all'
      : draft.pairs
          .map((pair) => pair.split('|'))
          .filter(([from, to]) => statusRefs.has(from) && statusRefs.has(to) && from !== to)
          .map(([from, to]) => ({ from, to })),
    ...(Object.keys(replacements).length ? { replacements } : {}),
  }
}
