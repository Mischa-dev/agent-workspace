// Lays out a dependency graph left to right: each task sits one column after
// the last task that blocks it, so work flows toward the right. Rows within a
// column follow the average row of a task's blockers, which keeps most lines
// short without a layout library.

export type LayoutInput = { nodes: { id: string }[]; edges: { from: string; to: string }[] }

export type Placed = { id: string; column: number; row: number; x: number; y: number }

export type Layout = {
  placed: Map<string, Placed>
  width: number
  height: number
}

export const NODE_WIDTH = 208
export const NODE_HEIGHT = 58
const COLUMN_GAP = 72
const ROW_GAP = 18
const PADDING = 16

export function layoutGraph({ nodes, edges }: LayoutInput): Layout {
  const ids = new Set(nodes.map((node) => node.id))
  const blockers = new Map<string, string[]>()
  for (const node of nodes) blockers.set(node.id, [])
  for (const edge of edges) {
    if (ids.has(edge.from) && ids.has(edge.to) && edge.from !== edge.to) blockers.get(edge.to)?.push(edge.from)
  }

  // Column = longest chain of blockers before it. A cycle can't happen for real
  // dependencies, but suggested edges might close one; `visiting` breaks it.
  const column = new Map<string, number>()
  const visiting = new Set<string>()
  const columnOf = (id: string): number => {
    const known = column.get(id)
    if (known !== undefined) return known
    if (visiting.has(id)) return 0
    visiting.add(id)
    let value = 0
    for (const blocker of blockers.get(id) ?? []) value = Math.max(value, columnOf(blocker) + 1)
    visiting.delete(id)
    column.set(id, value)
    return value
  }
  for (const node of nodes) columnOf(node.id)

  const columns: string[][] = []
  for (const node of nodes) {
    const index = column.get(node.id) ?? 0
    ;(columns[index] ??= []).push(node.id)
  }

  const row = new Map<string, number>()
  columns.forEach((members, index) => {
    if (index > 0) {
      // Order by where the blockers sit; ties keep the server's order (best first).
      const weight = (id: string) => {
        const rows = (blockers.get(id) ?? []).map((blocker) => row.get(blocker)).filter((value): value is number => value !== undefined)
        return rows.length ? rows.reduce((sum, value) => sum + value, 0) / rows.length : Number.MAX_SAFE_INTEGER
      }
      const order = new Map(members.map((id, position) => [id, position]))
      members.sort((a, b) => weight(a) - weight(b) || (order.get(a) ?? 0) - (order.get(b) ?? 0))
    }
    members.forEach((id, position) => row.set(id, position))
  })

  const placed = new Map<string, Placed>()
  let rows = 0
  columns.forEach((members, index) => {
    rows = Math.max(rows, members.length)
    members.forEach((id, position) => {
      placed.set(id, {
        id,
        column: index,
        row: position,
        x: PADDING + index * (NODE_WIDTH + COLUMN_GAP),
        y: PADDING + position * (NODE_HEIGHT + ROW_GAP),
      })
    })
  })
  return {
    placed,
    width: PADDING * 2 + Math.max(1, columns.length) * NODE_WIDTH + Math.max(0, columns.length - 1) * COLUMN_GAP,
    height: PADDING * 2 + Math.max(1, rows) * NODE_HEIGHT + Math.max(0, rows - 1) * ROW_GAP,
  }
}

/** A curved line from the right edge of one box to the left edge of another. */
export function edgePath(from: Placed, to: Placed) {
  const x1 = from.x + NODE_WIDTH
  const y1 = from.y + NODE_HEIGHT / 2
  const x2 = to.x
  const y2 = to.y + NODE_HEIGHT / 2
  if (x2 <= x1) {
    // Same or earlier column (only a suggested edge can do this): loop around below.
    const drop = Math.max(y1, y2) + NODE_HEIGHT
    return `M ${x1} ${y1} C ${x1 + 40} ${drop}, ${x2 - 40} ${drop}, ${x2} ${y2}`
  }
  const bend = Math.max(24, (x2 - x1) / 2)
  return `M ${x1} ${y1} C ${x1 + bend} ${y1}, ${x2 - bend} ${y2}, ${x2} ${y2}`
}
