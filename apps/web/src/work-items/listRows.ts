import type { WorkItemCard, WorkItemGroup } from './types'

/**
 * Turns the loaded pages of a (possibly grouped) list into the flat rows the
 * windowed table draws: group headers with their full counts, the tasks under
 * each open group, and one "loading" row where the next page will land.
 *
 * Pages arrive in group order (keyset paging), so every group before the last
 * loaded task is complete, the last one may be partial, and later groups are
 * known only from GET /work-items/groups until their tasks load.
 */

export type ListRow =
  | { kind: 'group'; key: string; groupKey: string; label: string; count: number | null; color?: string; collapsed: boolean; loaded: number }
  | { kind: 'task'; key: string; groupKey: string | null; item: WorkItemCard }
  | { kind: 'loading'; key: string }

export function taskRowKey(item: WorkItemCard) {
  return item.group ? `${item.group.key}:${item.id}` : item.id
}

export function buildListRows({
  collapsed,
  grouped,
  groups,
  hasMore,
  items,
}: {
  items: WorkItemCard[]
  grouped: boolean
  groups: WorkItemGroup[] | undefined
  collapsed: ReadonlySet<string>
  hasMore: boolean
}): ListRow[] {
  const loading: ListRow = { kind: 'loading', key: '__loading' }
  if (!grouped) {
    const rows: ListRow[] = []
    const seen = new Set<string>()
    for (const item of items) {
      if (seen.has(item.id)) continue
      seen.add(item.id)
      rows.push({ kind: 'task', key: item.id, groupKey: null, item })
    }
    if (hasMore) rows.push(loading)
    return rows
  }

  // Tasks by group, in arrival order, without duplicates inside a group.
  const byGroup = new Map<string, WorkItemCard[]>()
  const labels = new Map<string, string>()
  const arrival: string[] = []
  for (const item of items) {
    const groupKey = item.group?.key ?? 'none'
    let bucket = byGroup.get(groupKey)
    if (!bucket) {
      bucket = []
      byGroup.set(groupKey, bucket)
      arrival.push(groupKey)
      labels.set(groupKey, item.group?.label ?? 'None')
    }
    if (!bucket.some((existing) => existing.id === item.id)) bucket.push(item)
  }

  // Headers come from the groups endpoint; a group it doesn't know (a race
  // with a write) still shows, after the known ones.
  const order: Array<{ key: string; label: string; count: number | null; color?: string }> = (groups ?? []).map((group) => ({
    key: group.key,
    label: group.label,
    count: group.count,
    ...(group.color ? { color: group.color } : {}),
  }))
  const known = new Set(order.map((group) => group.key))
  for (const key of arrival) {
    if (!known.has(key)) order.push({ key, label: labels.get(key) ?? 'None', count: null })
  }
  const lastLoadedGroup = arrival[arrival.length - 1] ?? null

  const rows: ListRow[] = []
  let loadingPlaced = !hasMore
  for (const group of order) {
    const tasks = byGroup.get(group.key) ?? []
    const isCollapsed = collapsed.has(group.key)
    // Before anything has loaded, the next page starts at the first group.
    if (!loadingPlaced && lastLoadedGroup === null) {
      rows.push(loading)
      loadingPlaced = true
    }
    rows.push({
      kind: 'group',
      key: `group:${group.key}`,
      groupKey: group.key,
      label: group.label,
      count: group.count,
      ...(group.color ? { color: group.color } : {}),
      collapsed: isCollapsed,
      loaded: tasks.length,
    })
    if (!isCollapsed) {
      for (const item of tasks) rows.push({ kind: 'task', key: `${group.key}:${item.id}`, groupKey: group.key, item })
    }
    if (!loadingPlaced && group.key === lastLoadedGroup) {
      rows.push(loading)
      loadingPlaced = true
    }
  }
  if (!loadingPlaced) rows.push(loading)
  return rows
}

/** The tasks in the order they appear, once each, for keyboard movement and range selection. */
export function visibleTaskIds(rows: ListRow[]) {
  const ids: string[] = []
  const seen = new Set<string>()
  for (const row of rows) {
    if (row.kind !== 'task' || seen.has(row.item.id)) continue
    seen.add(row.item.id)
    ids.push(row.item.id)
  }
  return ids
}
