// What each event makes stale loads with the first connection, not the first
// paint, and once: every connection shares the one load. Tests wait for a
// load in flight before their page is torn down (src/test/setup.ts).
let table: Promise<typeof import('./table')> | undefined

export function loadTouchTable(): Promise<typeof import('./table')> {
  table ??= import('./table')
  return table
}

/** The load in flight or done, if one has started. */
export function touchTableLoading(): Promise<unknown> | undefined {
  return table
}
