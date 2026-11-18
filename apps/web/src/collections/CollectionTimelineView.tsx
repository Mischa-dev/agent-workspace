import { useEffect, useMemo } from 'react'
import { useSearchParams } from 'react-router-dom'

import type { FieldDefinition } from '../fields/fieldModel'
import { Banner, Button, EmptyState, ListSkeleton, countLabel } from '../ui'
import { addDays, dayFromNumber, dayLabel, dayNumber, today } from '../views/dates'
import { recordsParams, useRecords, type CollectionBody, type CollectionRecord, type RecordsQuery, type ViewConfig } from './api'
import { recordName, recordSpan, timelineWindow } from './collectionModel'
import '../views/views.css'

type Props = {
  body: CollectionBody
  config: ViewConfig
  onOpenRecord: (id: string) => void
  openRecordId: string | null
  query: RecordsQuery
  projectId: string
  recordHref: (id: string) => string
  dateFields: readonly FieldDefinition[]
  onDateField: (fieldId: string) => void
}

const DAY_WIDTH = 22
const ROW_HEIGHT = 34
const HEADER_HEIGHT = 40
const WEEKS = 13
/** Records one stretch shows at most; more say so. */
const MOST = 500

/**
 * A collection as a timeline (T1.14): each record a bar from the view's start
 * date field to its end date field (one day without one), thirteen weeks at
 * a time. The stretch is in the address (`?from=2026-11-02`). A bar opens its
 * record.
 */
export function CollectionTimelineView(props: Props) {
  const start = props.body.fields.find((entry) => entry.id === props.config.dateField && !entry.archived && entry.type === 'date')
  if (!start) {
    return (
      <EmptyState icon="clock" title="Pick the date the bars start on">
        {props.dateFields.length ? (
          <>
            A timeline draws each record from one date field to another.
            <span className="aw-cboard__pick">
              {props.dateFields.map((entry) => (
                <Button key={entry.id} onClick={() => props.onDateField(entry.id)} size="sm">
                  {entry.name}
                </Button>
              ))}
            </span>
          </>
        ) : (
          'A timeline draws each record from one date field to another. Add a date field to this collection first.'
        )}
      </EmptyState>
    )
  }
  const end = props.body.fields.find((entry) => entry.id === props.config.endField && entry.id !== start.id && !entry.archived && entry.type === 'date') ?? null
  return <Timeline {...props} end={end} start={start} />
}

function Timeline({ body, end, onOpenRecord, openRecordId, projectId, query, recordHref, start }: Props & { start: FieldDefinition; end: FieldDefinition | null }) {
  const [params, setParams] = useSearchParams()
  const anchor = params.get('from') ?? today()
  const window = timelineWindow(/^\d{4}-\d{2}-\d{2}$/.test(anchor) ? anchor : today(), WEEKS)
  const records = useRecords(
    projectId,
    body.collection.id,
    recordsParams({ ...query, groupBy: null, dates: { field: start.id, endField: end?.id ?? null, from: window.from, to: window.to } }),
    true,
    200,
  )
  const loaded = useMemo(() => records.data?.pages.flatMap((page) => page.records) ?? [], [records.data])
  const { fetchNextPage, hasNextPage, isFetchingNextPage, isFetchNextPageError } = records
  // A stretch's records all show, so the pages after the first load straight after it, up to MOST.
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage && !isFetchNextPageError && loaded.length < MOST) void fetchNextPage()
  }, [fetchNextPage, hasNextPage, isFetchNextPageError, isFetchingNextPage, loaded.length])

  const goTo = (from: string | null) =>
    setParams(
      (current) => {
        const updated = new URLSearchParams(current)
        if (from) updated.set('from', from)
        else updated.delete('from')
        return updated
      },
      { replace: true },
    )

  const rows = loaded.flatMap((record) => {
    const span = recordSpan(record, start.id, end?.id)
    return span ? [{ record, span }] : []
  })
  rows.sort((left, right) => left.span.start - right.span.start || left.span.end - right.span.end)

  const labelWidth = typeof globalThis.innerWidth === 'number' && globalThis.innerWidth < 640 ? 140 : 240
  const days = window.last - window.first + 1
  const x = (day: number) => labelWidth + (day - window.first) * DAY_WIDTH
  const todayDay = dayNumber(today())
  const months: { label: string; left: number; width: number }[] = []
  const weeks: number[] = []
  for (let day = window.first; day <= window.last; day += 1) {
    if (dayFromNumber(day).endsWith('-01') || day === window.first) {
      const previous = months.at(-1)
      if (previous) previous.width = x(day) - previous.left
      months.push({ label: new Date(day * 86_400_000).toLocaleDateString([], { month: 'short', year: 'numeric', timeZone: 'UTC' }), left: x(day), width: 0 })
    }
    if (new Date(day * 86_400_000).getUTCDay() === 1) weeks.push(x(day))
  }
  const lastMonth = months.at(-1)
  if (lastMonth) lastMonth.width = x(window.last + 1) - lastMonth.left
  const describe = (record: CollectionRecord, span: { start: number; end: number }) =>
    `${recordName(record)}, ${dayLabel(dayFromNumber(span.start))}${span.end > span.start ? ` to ${dayLabel(dayFromNumber(span.end))}` : ''}`

  return (
    <div className="aw-ctimeline">
      <div className="aw-timeline__toolbar">
        <Button onClick={() => goTo(addDays(window.from, -7 * 4 + 7))} size="sm" variant="ghost">
          Earlier
        </Button>
        <Button onClick={() => goTo(null)} size="sm" variant="ghost">
          Today
        </Button>
        <Button onClick={() => goTo(addDays(window.from, 7 * 4 + 7))} size="sm" variant="ghost">
          Later
        </Button>
        <p aria-live="polite" className="aw-records__summary">
          {records.isPending ? 'Loading…' : `${countLabel(rows.length, 'record')} from ${dayLabel(window.from)} to ${dayLabel(dayFromNumber(window.last))}`}
        </p>
      </div>
      {records.isError && !records.data ? (
        <EmptyState action={<Button onClick={() => void records.refetch()}>Try again</Button>} icon="circle-alert" title="The records didn’t load">
          Check the connection and try again.
        </EmptyState>
      ) : records.isPending ? (
        <ListSkeleton label="Loading records" rows={5} />
      ) : !rows.length ? (
        <EmptyState icon="clock" title="Nothing dated in these weeks">
          Records show here when their {start.name.toLocaleLowerCase()} {end ? `to ${end.name.toLocaleLowerCase()} ` : ''}falls in these weeks. Go earlier or later to see others.
        </EmptyState>
      ) : (
        <>
          {loaded.length >= MOST && hasNextPage ? (
            <Banner className="aw-page__banner" icon="info">
              This shows the first {MOST} records in these weeks. Filter the view to see the rest.
            </Banner>
          ) : null}
          <div className="aw-timeline__scroll" tabIndex={-1}>
            <div className="aw-timeline__chart" style={{ width: labelWidth + days * DAY_WIDTH, height: HEADER_HEIGHT + rows.length * ROW_HEIGHT }}>
              <div className="aw-timeline__head" style={{ height: HEADER_HEIGHT }}>
                <span className="aw-timeline__corner" style={{ width: labelWidth }} />
                {months.map((month) => (
                  <span className="aw-timeline__month" key={month.left} style={{ left: month.left, width: month.width }}>
                    <span className="aw-timeline__monthname" style={{ left: labelWidth }}>
                      {month.label}
                    </span>
                  </span>
                ))}
              </div>
              {weeks.map((left) => (
                <span aria-hidden="true" className="aw-timeline__week" key={left} style={{ left, top: HEADER_HEIGHT }} />
              ))}
              {todayDay >= window.first && todayDay <= window.last ? (
                <span aria-hidden="true" className="aw-timeline__today" style={{ left: x(todayDay), top: HEADER_HEIGHT - 6 }} title="Today" />
              ) : null}
              <ul aria-label={body.collection.name} className="aw-timeline__rows" style={{ top: HEADER_HEIGHT }}>
                {rows.map(({ record, span }) => {
                  const from = Math.max(span.start, window.first)
                  const to = Math.min(span.end, window.last)
                  return (
                    <li aria-current={record.id === openRecordId || undefined} className="aw-timeline__row" key={record.id} style={{ height: ROW_HEIGHT }}>
                      <span className="aw-timeline__label" style={{ width: labelWidth }} title={recordName(record)}>
                        <span className="aw-timeline__key">#{record.number}</span>
                        <span className="aw-timeline__name">{record.title || 'Untitled'}</span>
                      </span>
                      <a
                        aria-label={describe(record, span)}
                        className="aw-timeline__bar aw-ctimeline__bar"
                        href={recordHref(record.id)}
                        onClick={(event) => {
                          if (event.metaKey || event.ctrlKey || event.shiftKey) return
                          event.preventDefault()
                          onOpenRecord(record.id)
                        }}
                        style={{ left: x(from), width: Math.max(DAY_WIDTH, (to - from + 1) * DAY_WIDTH) }}
                        title={describe(record, span)}
                      />
                    </li>
                  )
                })}
              </ul>
            </div>
          </div>
        </>
      )}
    </div>
  )
}
