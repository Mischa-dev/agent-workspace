// Saved filter views and find-and-replace helpers for the sheet page (C1.16).
import { useQuery, type QueryClient } from '@tanstack/react-query'

import { callOp, opPrefix, opQuery, type OpOutput } from '../api/ops'
import type { ViewSpec } from './sheetModel'

export type SheetView = OpOutput<'sheet_view_list'>['views'][number]

/** The saved views of a sheet's tab: the shared ones and your own. */
export function useSheetViews(sheetId: string, tabId: string | undefined, enabled = true) {
  return useQuery({ ...opQuery('sheet_view_list', { sheetId, tab: tabId }), enabled: enabled && Boolean(tabId) })
}

/** The view as the grid applies it. */
export function viewSpec(view: SheetView): ViewSpec {
  return {
    filters: view.filters.map((filter) => ({ column: filter.column, op: filter.op, value: filter.value, values: filter.values })),
    sort: view.sort,
    hiddenColumns: view.hiddenColumns,
  }
}

const escapeRegExp = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

/** The input after replacing what was looked for. */
export function replaced(input: string, query: string, replacement: string, options: { matchCase: boolean; wholeCell: boolean }) {
  if (options.wholeCell) return replacement
  return input.replace(new RegExp(escapeRegExp(query), options.matchCase ? 'g' : 'gi'), () => replacement)
}

/** Removes a saved view. */
export async function deleteSheetView(client: QueryClient, view: SheetView) {
  await callOp('sheet_view_delete', { viewId: view.id })
  await client.invalidateQueries({ queryKey: opPrefix('sheet_view_list') })
}
