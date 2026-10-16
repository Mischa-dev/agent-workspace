/**
 * Which tasks are selected for a bulk change. Selection is by task id, so a
 * task shown twice (grouped by label) is one selection. Shift extends a range
 * from the last task picked; Cmd or Ctrl adds or removes one.
 */
export type Selection = { ids: ReadonlySet<string>; anchor: string | null }

export const emptySelection: Selection = { ids: new Set(), anchor: null }

export function toggleOne(selection: Selection, id: string): Selection {
  const ids = new Set(selection.ids)
  if (ids.has(id)) ids.delete(id)
  else ids.add(id)
  return { ids, anchor: id }
}

/** Adds every task between the anchor and `id`, in the order they're shown. */
export function selectRange(selection: Selection, id: string, order: readonly string[]): Selection {
  const anchor = selection.anchor && order.includes(selection.anchor) ? selection.anchor : null
  if (!anchor) return toggleOne(selection, id)
  const from = order.indexOf(anchor)
  const to = order.indexOf(id)
  if (to < 0) return selection
  const ids = new Set(selection.ids)
  const [start, end] = from <= to ? [from, to] : [to, from]
  for (const candidate of order.slice(start, end + 1)) ids.add(candidate)
  return { ids, anchor }
}

/** A pointer pick: Shift for a range, Cmd/Ctrl (or the checkbox) to toggle one. */
export function pick(
  selection: Selection,
  id: string,
  order: readonly string[],
  modifiers: { shift?: boolean; toggle?: boolean },
): Selection {
  if (modifiers.shift) return selectRange(selection, id, order)
  return toggleOne(selection, id)
}

/** The most tasks one bulk change takes, as the server allows. */
export const MAX_BULK = 200

/** Every loaded task, up to the first `MAX_BULK`. */
export function selectAll(order: readonly string[]): Selection {
  const ids = order.slice(0, MAX_BULK)
  return { ids: new Set(ids), anchor: ids[ids.length - 1] ?? null }
}

/** Drops ids that are no longer loaded, for example after a filter change. */
export function keepLoaded(selection: Selection, loaded: ReadonlySet<string>): Selection {
  let changed = false
  const ids = new Set<string>()
  for (const id of selection.ids) {
    if (loaded.has(id)) ids.add(id)
    else changed = true
  }
  if (!changed) return selection
  return { ids, anchor: selection.anchor && loaded.has(selection.anchor) ? selection.anchor : null }
}
