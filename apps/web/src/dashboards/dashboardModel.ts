import type { DashboardEntry, Figures, TileEntry, TileInput } from '../api/generated/operations'
import type { ChartPoint } from '../collections/CollectionChartView'
import { projectAreaHref } from '../projects/locations'

export const dashboardsHref = (projectId: string) => projectAreaHref(projectId, 'dashboards')
export const dashboardHref = (projectId: string, id: string) => projectAreaHref(projectId, 'dashboards', id)

/** Where a view tile leads: its collection, opened on its view. */
export function tileHref(projectId: string, tile: Pick<TileEntry, 'collection' | 'view'>) {
  return tile.collection ? projectAreaHref(projectId, 'collections', tile.collection, tile.view ? { view: tile.view } : undefined) : null
}

/** A tile's widths, in columns of twelve: a quarter, a third, a half, two thirds, all. */
export const TILE_WIDTHS = [3, 4, 6, 8, 12] as const
export const MOST_HEIGHT = 4
export const MOST_TILES = 24

/** The tiles as `dashboard_update` takes them back, unchanged. */
export function tileInputs(tiles: readonly TileEntry[]): TileInput[] {
  return tiles.map((tile) => ({
    id: tile.id,
    kind: tile.kind,
    title: tile.title || null,
    view: tile.kind === 'view' ? (tile.view ?? null) : null,
    text: tile.kind === 'text' ? tile.text : null,
    width: tile.width,
    height: tile.height,
  }))
}

/** The tiles with the one at `index` moved `by` places (clamped to the ends). */
export function moveTile<T>(tiles: readonly T[], index: number, by: number): T[] {
  const to = Math.max(0, Math.min(tiles.length - 1, index + by))
  if (to === index || index < 0 || index >= tiles.length) return [...tiles]
  const next = [...tiles]
  const [moved] = next.splice(index, 1)
  next.splice(to, 0, moved)
  return next
}

/** The next width up or down from `width`, staying within the widths there are. */
export function stepWidth(width: number, by: 1 | -1): number {
  const at = TILE_WIDTHS.findIndex((candidate) => candidate >= width)
  const index = at === -1 ? TILE_WIDTHS.length - 1 : at
  return TILE_WIDTHS[Math.max(0, Math.min(TILE_WIDTHS.length - 1, index + by))]
}

/** A height one row taller or shorter, 1 to 4 rows. */
export function stepHeight(height: number, by: 1 | -1): number {
  return Math.max(1, Math.min(MOST_HEIGHT, height + by))
}

/** What a tile's width is called. */
export function widthLabel(width: number) {
  return width >= 12 ? 'Full width' : width >= 8 ? 'Two thirds' : width >= 6 ? 'Half' : width >= 4 ? 'A third' : 'A quarter'
}

/** The words for a chart's measure from the server ("count", "sum of Hours"). */
export function measureWords(measure: string | null | undefined) {
  if (!measure || measure === 'count') return 'Records'
  const [op, ...field] = measure.split(' of ')
  const words: Record<string, string> = { sum: 'Total', average: 'Average', min: 'Least', max: 'Most' }
  return `${words[op] ?? op} ${field.join(' of ')}`.trim()
}

/** A chart tile's groups as points, its count or (with a measure) its value. */
export function figurePoints(figures: Figures): ChartPoint[] {
  const counting = !figures.measure || figures.measure === 'count'
  return figures.groups.map((group) => ({
    key: group.key,
    label: group.name || 'None',
    color: group.color ?? null,
    count: group.count,
    value: counting ? group.count : (group.value ?? null),
  }))
}

/** Whether a measure's values add up to a whole (a count or a sum). */
export function additive(measure: string | null | undefined) {
  return !measure || measure === 'count' || measure.startsWith('sum ')
}

/** Shared dashboards first, then your own, each in order. */
export function splitDashboards(dashboards: readonly DashboardEntry[]) {
  return { shared: dashboards.filter((entry) => entry.shared), own: dashboards.filter((entry) => !entry.shared) }
}
