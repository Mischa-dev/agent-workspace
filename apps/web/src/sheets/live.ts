import type { LiveHandler } from '../modules/define'
import { COLLECTION_DELAY_MS, isCommentSummary, isOpKey, later, sheetChange } from '../realtime/liveDocuments'
import { sheetKeys } from './keys'

/** Whether a dashboard read through `dashboard_get` has a tile over this sheet. */
function chartsSheet(data: unknown, sheetId: string) {
  const tiles = (data as { tiles?: { sheet?: unknown }[] } | undefined)?.tiles
  return Array.isArray(tiles) && tiles.some((tile) => tile.sheet === sheetId)
}

/**
 * A sheet.updated event (M1.5): the open sheet and the space's sheet list
 * refetch once for a burst of changes, its cell discussions too (their
 * anchors may have moved with rows and columns), and any dashboard charting
 * it (C1.20), and its charts (C1.18).
 */
const sheetChanged: LiveHandler = (queryClient, event) => {
  const sheet = sheetChange(event)
  if (!sheet) return
  later(`sheet:${event.projectId}:${sheet.sheetId}`, COLLECTION_DELAY_MS, () => {
    void queryClient.invalidateQueries({ queryKey: sheetKeys.one(event.projectId, sheet.sheetId) })
    void queryClient.invalidateQueries({ queryKey: sheetKeys.list(event.projectId) })
    void queryClient.invalidateQueries({ predicate: (query) => isCommentSummary(query.queryKey, sheet.sheetId) })
    void queryClient.invalidateQueries({ predicate: (query) => isOpKey(query.queryKey, 'dashboard_get') && chartsSheet(query.state.data, sheet.sheetId) })
    // Its charts, in the grid and in pages, read their cells again (C1.18).
    void queryClient.invalidateQueries({
      predicate: (query) => isOpKey(query.queryKey, 'sheet_chart_get') && (query.queryKey[3] as { sheetId?: unknown } | undefined)?.sheetId === sheet.sheetId,
    })
  })
}

export default sheetChanged
