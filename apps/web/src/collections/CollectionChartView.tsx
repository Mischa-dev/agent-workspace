import { useMemo } from 'react'

import type { FieldDefinition } from '../fields/fieldModel'
import { Button, EmptyState, ListSkeleton, SWATCHES, countLabel } from '../ui'
import { recordsParams, useRecords, type CollectionBody, type RecordsQuery, type ViewConfig } from './api'
import { chartNumber, chartPoints, chartTicks, measureLabel } from './collectionModel'

type Point = ReturnType<typeof chartPoints>[number]

/** Bars a chart draws at most; the rest say so (and are in the table). */
const MOST_BARS = 50
/** A donut's slices at most, the rest one "Other" slice. */
const MOST_SLICES = 7

/**
 * A collection as a chart (T1.31): the view's groups as bars, a line or a
 * donut, each showing how many records it has or what a number field adds
 * up to there; or one number over every record. The view's filters hold.
 * The figures are also a table, for reading them out.
 */
export function CollectionChartView({
  body,
  config,
  filtered,
  groupable,
  onClearFilters,
  onGroupBy,
  projectId,
  query,
}: {
  body: CollectionBody
  config: ViewConfig
  filtered: boolean
  groupable: readonly FieldDefinition[]
  onClearFilters: () => void
  onGroupBy: (fieldId: string) => void
  projectId: string
  query: RecordsQuery
}) {
  const chart = config.chart ?? { style: 'bar' as const, measure: { op: 'count' as const } }
  const op = chart.measure.field ? chart.measure.op : 'count'
  const grouped = Boolean(config.groupBy) && chart.style !== 'number'
  const params = recordsParams({ ...query, groupBy: grouped ? config.groupBy : null, measure: { op, field: chart.measure.field ?? null } })
  // Only the figures are read: one record comes along with them.
  const records = useRecords(projectId, body.collection.id, params, true, 1)
  const page = records.data?.pages[0]
  const points = useMemo(() => chartPoints(page?.groups ?? [], op), [page, op])
  const label = measureLabel(chart, body.fields)
  const groupField = body.fields.find((field) => field.id === config.groupBy)

  if (records.isPending) return <ListSkeleton label="Loading the chart" rows={4} />
  if (records.isError && !page) {
    return (
      <EmptyState action={<Button onClick={() => void records.refetch()}>Try again</Button>} icon="circle-alert" title="The chart didn’t load">
        Check the connection and try again.
      </EmptyState>
    )
  }
  const total = page?.total ?? 0
  if (total === 0) {
    return (
      <EmptyState action={filtered ? <Button onClick={onClearFilters}>Clear filters</Button> : null} icon="chart" title={filtered ? 'No records match these filters' : 'No records to chart yet'}>
        {filtered ? 'Clear the filters to chart every record.' : 'The chart fills in as records are added.'}
      </EmptyState>
    )
  }
  const overall = op === 'count' ? total : (page?.value ?? null)
  if (!grouped) {
    return (
      <div className="aw-cchart">
        <figure className="aw-cchart__hero">
          <span className="aw-cchart__heronumber">{overall === null ? '—' : chartNumber(overall)}</span>
          <figcaption className="aw-cchart__herolabel">
            {label}
            {op === 'count' ? '' : ` across ${countLabel(total, 'record')}`}
          </figcaption>
        </figure>
        {chart.style !== 'number' && groupable.length ? (
          <p className="aw-cchart__pick">
            Group the records to chart them side by side:
            {groupable.slice(0, 6).map((field) => (
              <Button key={field.id} onClick={() => onGroupBy(field.id)} size="sm">
                {field.name}
              </Button>
            ))}
          </p>
        ) : null}
      </div>
    )
  }
  const title = `${label} by ${groupField?.name.toLocaleLowerCase() ?? 'group'}`
  return (
    <div className="aw-cchart">
      <p aria-live="polite" className="aw-records__summary">
        {title}
        {overall === null ? '' : `: ${chartNumber(overall)} in all`}
        {op === 'count' ? '' : `, from ${countLabel(total, 'record')}`}
      </p>
      {chart.style === 'line' ? <LineChart label={title} points={points} /> : chart.style === 'donut' ? <DonutChart additive={op === 'count' || op === 'sum'} label={title} points={points} /> : <BarChart label={title} points={points} />}
      <details className="aw-cchart__table">
        <summary>Show as a table</summary>
        <table>
          <thead>
            <tr>
              <th scope="col">{groupField?.name ?? 'Group'}</th>
              <th scope="col">Records</th>
              {op === 'count' ? null : <th scope="col">{label}</th>}
            </tr>
          </thead>
          <tbody>
            {points.map((point) => (
              <tr key={point.key}>
                <th scope="row">{point.label}</th>
                <td>{point.count.toLocaleString()}</td>
                {op === 'count' ? null : <td>{point.value === null ? '—' : chartNumber(point.value)}</td>}
              </tr>
            ))}
          </tbody>
        </table>
      </details>
    </div>
  )
}

function BarChart({ label, points }: { label: string; points: readonly Point[] }) {
  const shown = points.slice(0, MOST_BARS)
  const values = shown.map((point) => point.value ?? 0)
  const most = Math.max(0, ...values)
  const least = Math.min(0, ...values)
  const span = most - least || 1
  return (
    <>
      <ul aria-label={label} className="aw-cchart__bars">
        {shown.map((point) => {
          const value = point.value ?? 0
          const start = ((Math.min(0, value) - least) / span) * 100
          const width = (Math.abs(value) / span) * 100
          const text = point.value === null ? '—' : chartNumber(point.value)
          return (
            <li className="aw-cchart__bar" key={point.key} title={`${point.label}: ${text}`}>
              <span className="aw-cchart__barlabel">{point.label}</span>
              <span className="aw-cchart__track">
                <span className={`aw-cchart__fill${value < 0 ? ' aw-cchart__fill--below' : ''}`} style={{ left: `${start}%`, width: `${width}%` }} />
              </span>
              <span className="aw-cchart__barvalue">{text}</span>
            </li>
          )
        })}
      </ul>
      {points.length > shown.length ? <p className="aw-cchart__more">{countLabel(points.length - shown.length, 'more group')} in the table below.</p> : null}
    </>
  )
}

const LINE_HEIGHT = 220
const LINE_STEP = 64
const LINE_LEFT = 56
const LINE_BOTTOM = 36

function LineChart({ label, points }: { label: string; points: readonly Point[] }) {
  const values = points.flatMap((point) => (point.value === null ? [] : [point.value]))
  const ticks = chartTicks(Math.min(0, ...values), Math.max(0, ...values))
  const low = ticks[0] ?? 0
  const high = ticks.at(-1) ?? 1
  const plot = LINE_HEIGHT - LINE_BOTTOM - 12
  const y = (value: number) => 12 + plot - ((value - low) / (high - low || 1)) * plot
  const x = (index: number) => LINE_LEFT + LINE_STEP / 2 + index * LINE_STEP
  const width = LINE_LEFT + points.length * LINE_STEP + 8
  // Labels thin out so they don't run into each other.
  const every = Math.max(1, Math.ceil(points.reduce((longest, point) => Math.max(longest, point.label.length), 0) * 7 / LINE_STEP))
  const path = points
    .map((point, index) => (point.value === null ? null : `${x(index)},${y(point.value)}`))
    .reduce<string[]>((parts, at, index) => (at === null ? parts : [...parts, `${parts.length && points[index - 1]?.value !== null ? 'L' : 'M'}${at}`]), [])
    .join(' ')
  return (
    <div className="aw-cchart__scroll">
      <svg aria-label={label} className="aw-cchart__line" height={LINE_HEIGHT} role="img" viewBox={`0 0 ${width} ${LINE_HEIGHT}`} width={width}>
        {ticks.map((tick) => (
          <g key={tick}>
            <line className="aw-cchart__grid" x1={LINE_LEFT} x2={width - 8} y1={y(tick)} y2={y(tick)} />
            <text className="aw-cchart__axis" textAnchor="end" x={LINE_LEFT - 8} y={y(tick) + 4}>
              {chartNumber(tick, true)}
            </text>
          </g>
        ))}
        <path className="aw-cchart__path" d={path} />
        {points.map((point, index) => (
          <g key={point.key}>
            {index % every === 0 ? (
              <text className="aw-cchart__axis" textAnchor="middle" x={x(index)} y={LINE_HEIGHT - 12}>
                {point.label.length > 14 ? `${point.label.slice(0, 13)}…` : point.label}
              </text>
            ) : null}
            {point.value === null ? null : (
              <circle className="aw-cchart__dot" cx={x(index)} cy={y(point.value)} r={4}>
                <title>{`${point.label}: ${chartNumber(point.value)}`}</title>
              </circle>
            )}
          </g>
        ))}
      </svg>
    </div>
  )
}

const RADIUS = 64
const RING = 22

function DonutChart({ additive, label, points }: { additive: boolean; label: string; points: readonly Point[] }) {
  const positive = points.filter((point) => (point.value ?? 0) > 0)
  // Counts and totals fold the smallest into "Other"; an average or a least
  // doesn't add up, so the rest are only left out.
  const ranked = [...positive].sort((left, right) => (right.value ?? 0) - (left.value ?? 0))
  const kept = ranked.length > MOST_SLICES + 1 ? ranked.slice(0, MOST_SLICES) : ranked
  const rest = ranked.slice(kept.length)
  const slices = [
    ...positive.filter((point) => kept.includes(point)).map((point, index) => ({ key: point.key, label: point.label, value: point.value ?? 0, color: `var(--swatch-${SWATCHES[index % SWATCHES.length]})` })),
    ...(rest.length && additive ? [{ key: 'other', label: `Other (${rest.length})`, value: rest.reduce((sum, point) => sum + (point.value ?? 0), 0), color: 'var(--content-fg-muted)' }] : []),
  ]
  const whole = slices.reduce((sum, slice) => sum + slice.value, 0) || 1
  const circumference = 2 * Math.PI * RADIUS
  const starts = slices.map((_, index) => slices.slice(0, index).reduce((sum, slice) => sum + (slice.value / whole) * circumference, 0))
  return (
    <div className="aw-cchart__donut">
      <svg aria-label={label} height={(RADIUS + RING) * 2} role="img" viewBox={`0 0 ${(RADIUS + RING) * 2} ${(RADIUS + RING) * 2}`} width={(RADIUS + RING) * 2}>
        <g transform={`rotate(-90 ${RADIUS + RING} ${RADIUS + RING})`}>
          {slices.map((slice, index) => {
            const length = (slice.value / whole) * circumference
            const drawn = slices.length > 1 ? Math.max(0, length - 2) : length
            return (
              <circle
                cx={RADIUS + RING}
                cy={RADIUS + RING}
                fill="none"
                key={slice.key}
                r={RADIUS}
                stroke={slice.color}
                strokeDasharray={`${drawn} ${circumference - drawn}`}
                strokeDashoffset={-(starts[index] ?? 0)}
                strokeWidth={RING}
              >
                <title>{`${slice.label}: ${chartNumber(slice.value)}`}</title>
              </circle>
            )
          })}
        </g>
      </svg>
      <ul aria-label="Legend" className="aw-cchart__legend">
        {slices.map((slice) => (
          <li key={slice.key}>
            <span aria-hidden="true" className="aw-cchart__swatch" style={{ background: slice.color }} />
            <span className="aw-cchart__legendlabel">{slice.label}</span>
            <span className="aw-cchart__legendvalue">
              {chartNumber(slice.value)}
              {additive ? ` · ${Math.round((slice.value / whole) * 100)}%` : ''}
            </span>
          </li>
        ))}
        {rest.length && !additive ? <li className="aw-cchart__more">{countLabel(rest.length, 'more group')} in the table below.</li> : null}
        {positive.length < points.length ? <li className="aw-cchart__more">Groups at zero or below aren’t slices.</li> : null}
      </ul>
    </div>
  )
}
