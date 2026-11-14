import { useCallback, useMemo, useState } from 'react'

/**
 * What's selected in a table, list or board: some ids, the one a Shift-click
 * ranges from, or every row that matches the view (more than are loaded).
 */
export type Selection = {
  ids: ReadonlySet<string>
  anchor: string | null
  allMatching: boolean
}

export const NO_SELECTION: Selection = Object.freeze({ ids: new Set<string>(), anchor: null, allMatching: false })

/** Plain click: just this one. Cmd or Ctrl: toggle it. Shift: everything from the anchor to it. */
export type SelectMode = 'single' | 'toggle' | 'range'

export function select(current: Selection, order: readonly string[], id: string, mode: SelectMode): Selection {
  if (mode === 'single') return { ids: new Set([id]), anchor: id, allMatching: false }
  if (mode === 'toggle') {
    const ids = new Set(current.ids)
    if (ids.has(id)) ids.delete(id)
    else ids.add(id)
    return { ids, anchor: id, allMatching: false }
  }
  const from = current.anchor ? order.indexOf(current.anchor) : -1
  const to = order.indexOf(id)
  if (from < 0 || to < 0) return { ids: new Set([id]), anchor: id, allMatching: false }
  const [start, end] = from < to ? [from, to] : [to, from]
  const ids = new Set(current.ids)
  for (const each of order.slice(start, end + 1)) ids.add(each)
  return { ids, anchor: current.anchor, allMatching: false }
}

/** Every loaded row, or (when the view has more) every row that matches. */
export function selectAll(order: readonly string[], allMatching = false): Selection {
  return { ids: new Set(order), anchor: order[0] ?? null, allMatching }
}

export function isSelected(selection: Selection, id: string) {
  return selection.allMatching || selection.ids.has(id)
}

/** Rows that left the view leave the selection. */
export function keepOnly(selection: Selection, order: readonly string[]): Selection {
  if (selection.allMatching) return selection
  const visible = new Set(order)
  const ids = new Set([...selection.ids].filter((id) => visible.has(id)))
  if (ids.size === selection.ids.size) return selection
  return { ids, anchor: selection.anchor && visible.has(selection.anchor) ? selection.anchor : null, allMatching: false }
}

/** The selection model a piece takes. */
export type SelectionModel = {
  selection: Selection
  isSelected: (id: string) => boolean
  select: (id: string, mode?: SelectMode) => void
  selectAll: (allMatching?: boolean) => void
  clear: () => void
  count: number
}

export function useSelection(order: readonly string[]): SelectionModel {
  const [stored, setSelection] = useState<Selection>(NO_SELECTION)
  const selection = useMemo(() => keepOnly(stored, order), [stored, order])
  const choose = useCallback(
    (id: string, mode: SelectMode = 'single') => setSelection((current) => select(keepOnly(current, order), order, id, mode)),
    [order],
  )
  return useMemo(
    () => ({
      selection,
      isSelected: (id: string) => isSelected(selection, id),
      select: choose,
      selectAll: (allMatching = false) => setSelection(selectAll(order, allMatching)),
      clear: () => setSelection(NO_SELECTION),
      count: selection.ids.size,
    }),
    [selection, choose, order],
  )
}

/** The mode a pointer or key event asks for. */
export function selectModeOf(event: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }): SelectMode {
  if (event.shiftKey) return 'range'
  if (event.metaKey || event.ctrlKey) return 'toggle'
  return 'single'
}
