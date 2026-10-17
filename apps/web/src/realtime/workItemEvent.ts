/**
 * Reading work_item.updated events. Kept apart from the patches themselves
 * (workItemPatches.ts), which load only when such an event arrives.
 */

/** Where a task's card sits on the board, from a work_item.updated event. */
export type WorkItemPlacement = {
  statusId: string
  archived: boolean
  afterId: string | null
  beforeId: string | null
}

export type WorkItemChange = {
  workItemId: string
  version: number
  placement: WorkItemPlacement | null
}

const uuidPattern =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/

function uuid(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const normalized = value.toLowerCase()
  return uuidPattern.test(normalized) ? normalized : null
}

function optionalUuid(value: unknown): string | null | undefined {
  if (value === undefined || value === null) return null
  return uuid(value) ?? undefined
}

/** Reads a work_item.updated event. A malformed placement is dropped, not trusted. */
export function workItemChange(event: {
  type: string
  payload: Record<string, unknown>
}): WorkItemChange | null {
  if (event.type !== 'work_item.updated') return null
  const workItemId = uuid(event.payload.workItemId)
  const version = event.payload.version
  if (!workItemId || !Number.isSafeInteger(version) || (version as number) < 1) return null
  const raw = event.payload.placement
  let placement: WorkItemPlacement | null = null
  if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
    const record = raw as Record<string, unknown>
    const statusId = uuid(record.statusId)
    const afterId = optionalUuid(record.afterId)
    const beforeId = optionalUuid(record.beforeId)
    if (statusId && typeof record.archived === 'boolean' && afterId !== undefined && beforeId !== undefined) {
      placement = { statusId, archived: record.archived, afterId, beforeId }
    }
  }
  return { workItemId, version: version as number, placement }
}
