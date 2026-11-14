import { ChevronLeft } from 'lucide-react'
import { useEffect, useMemo, useRef, type ReactNode } from 'react'
import { useSearchParams } from 'react-router-dom'

import type { FieldDefinition } from '../fields/fieldModel'
import { Banner, Button, EmptyState, IconButton, ListSkeleton, countLabel } from '../ui'
import { addDays, addMonths, monthGrid, toIsoDate } from '../ui/dateModel'
import { useBoardDrag } from '../ui/useBoardDrag'
import { usePhoneWidth } from '../ui/useCoarsePointer'
import { recordsParams, useRecords, type CollectionBody, type CollectionRecord, type RecordsQuery, type ViewConfig } from './api'
import { calendarMonth, monthKey, recordName, recordsByDay } from './collectionModel'
import type { RecordEdits } from './useRecordEdits'

/** Records a day shows before "N more". */
const DAY_LIMIT = 3

type Props = {
  body: CollectionBody
  config: ViewConfig
  editable: boolean
  edits: RecordEdits
  onOpenRecord: (id: string) => void
  openRecordId: string | null
  query: RecordsQuery
  projectId: string
  recordHref: (id: string) => string
  dateFields: readonly FieldDefinition[]
  onDateField: (fieldId: string) => void
}

/**
 * A collection as a calendar (AWD-110): a month, each record on the day its
 * date field holds. The month is in the address (`?month=2026-11`). A day's
 * + adds a record on it, and dragging a record to another day changes its
 * date. On a phone the month is a list of the days that have records.
 */
export function RecordCalendar(props: Props) {
  const field = props.body.fields.find((entry) => entry.id === props.config.dateField && !entry.archived && entry.type === 'date')
  if (!field) {
    return (
      <EmptyState icon="clock" title="Pick the date to show records by">
        {props.dateFields.length ? (
          <>
            A calendar puts each record on the day a date field holds.
            <span className="aw-cboard__pick">
              {props.dateFields.map((entry) => (
                <Button key={entry.id} onClick={() => props.onDateField(entry.id)} size="sm">
                  {entry.name}
                </Button>
              ))}
            </span>
          </>
        ) : (
          'A calendar puts each record on the day a date field holds. Add a date field to this collection first.'
        )}
      </EmptyState>
    )
  }
  return <Calendar {...props} field={field} />
}

const WEEKDAYS = Array.from({ length: 7 }, (_, index) => new Date(2024, 0, 1 + index).toLocaleDateString([], { weekday: 'short' }))

function Calendar({ body, editable, edits, field, onOpenRecord, openRecordId, projectId, query, recordHref }: Props & { field: FieldDefinition }) {
  const [params, setParams] = useSearchParams()
  const phone = usePhoneWidth()
  const month = calendarMonth(params.get('month'))
  const days = useMemo(() => monthGrid(month), [month])
  const today = toIsoDate(new Date())
  const from = toIsoDate(days[0])
  const to = toIsoDate(addDays(days[days.length - 1], 1))
  const records = useRecords(projectId, body.collection.id, recordsParams({ ...query, groupBy: null, dates: { field: field.id, from, to } }), true, 200)
  const loaded = useMemo(() => records.data?.pages.flatMap((page) => page.records) ?? [], [records.data])
  const byDay = useMemo(() => recordsByDay(loaded, field.id), [field.id, loaded])
  const ghostRef = useRef<HTMLDivElement>(null)
  const scrollerRef = useRef<HTMLDivElement>(null)
  const drag = useBoardDrag({
    enabled: editable && !phone,
    ghostRef,
    scrollerRef,
    onDrop: (id, target) => {
      const record = loaded.find((entry) => entry.id === id)
      if (record && record.values[field.id] !== target.columnKey) void edits.save(record, { values: { [field.id]: target.columnKey } })
    },
  })
  const dragged = drag.drag ? loaded.find((entry) => entry.id === drag.drag?.cardId) : undefined
  const { fetchNextPage, hasNextPage, isFetchingNextPage, isFetchNextPageError } = records
  // A month's records all show, so the pages after the first load straight after it.
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage && !isFetchNextPageError) void fetchNextPage()
  }, [fetchNextPage, hasNextPage, isFetchNextPageError, isFetchingNextPage])

  const goTo = (next: Date | null) =>
    setParams(
      (current) => {
        const updated = new URLSearchParams(current)
        if (next && monthKey(next) !== monthKey(new Date())) updated.set('month', monthKey(next))
        else updated.delete('month')
        return updated
      },
      { replace: true },
    )
  const addOn = async (day: string) => {
    const added = await edits.add('', { [field.id]: day })
    if (added) onOpenRecord(added.id)
  }
  const title = month.toLocaleDateString([], { month: 'long', year: 'numeric' })
  const inMonth = (day: Date) => day.getMonth() === month.getMonth()

  const recordLink = (record: CollectionRecord): ReactNode => (
    <a
      aria-current={record.id === openRecordId || undefined}
      className="aw-ccal__record"
      data-drag-id={record.id}
      href={recordHref(record.id)}
      key={record.id}
      onClick={(event) => {
        if (event.metaKey || event.ctrlKey || event.shiftKey) return
        event.preventDefault()
        if (!drag.justDragged()) onOpenRecord(record.id)
      }}
      onPointerDown={editable && !phone ? (event) => drag.onPointerDown(event, record.id) : undefined}
    >
      {record.title || `Untitled #${record.number}`}
    </a>
  )

  return (
    <div className="aw-ccal">
      <div className="aw-ccal__bar">
        <IconButton icon={ChevronLeft} label="Last month" onClick={() => goTo(addMonths(month, -1))} size="sm" />
        <h2 className="aw-ccal__month">{title}</h2>
        <IconButton icon="chevron-right" label="Next month" onClick={() => goTo(addMonths(month, 1))} size="sm" />
        <Button onClick={() => goTo(null)} size="sm" variant="ghost">
          Today
        </Button>
        <span aria-live="polite" className="aw-records__summary">
          {records.isPending ? 'Loading…' : `${countLabel(records.data?.pages[0]?.total ?? 0, 'record')} by ${field.name}`}
        </span>
      </div>
      {isFetchNextPageError ? (
        // A busy month loads in pages; one that failed says so rather than leave days looking empty (#244 review).
        <Banner
          action={
            <Button busy={isFetchingNextPage} onClick={() => void fetchNextPage()} size="sm">
              Load the rest
            </Button>
          }
          tone="warning"
        >
          Only {countLabel(loaded.length, 'record')} of {records.data?.pages[0]?.total ?? loaded.length} loaded for this month.
        </Banner>
      ) : null}
      {records.isPending ? (
        <ListSkeleton label="Loading the month" rows={5} />
      ) : records.isError && !records.data ? (
        <EmptyState action={<Button onClick={() => void records.refetch()}>Try again</Button>} icon="circle-alert" title="The month didn’t load">
          Check the connection and try again.
        </EmptyState>
      ) : phone ? (
        <PhoneAgenda byDay={byDay} days={days.filter(inMonth)} editable={editable} onAdd={addOn} recordLink={recordLink} today={today} />
      ) : (
        <div className={drag.drag ? 'aw-ccal__grid is-dragging' : 'aw-ccal__grid'} ref={scrollerRef}>
          <div aria-hidden="true" className="aw-ccal__weekdays">
            {WEEKDAYS.map((name) => (
              <span className="aw-ccal__weekday" key={name}>
                {name}
              </span>
            ))}
          </div>
          {Array.from({ length: days.length / 7 }, (_, week) => (
            <div className="aw-ccal__week" key={week}>
              {days.slice(week * 7, week * 7 + 7).map((day) => {
                const iso = toIsoDate(day)
                const entries = byDay.get(iso) ?? []
                const target = drag.drag?.target?.columnKey === iso
                return (
                  <div
                    aria-label={day.toLocaleDateString([], { weekday: 'long', month: 'long', day: 'numeric' })}
                    className={['aw-ccal__day', inMonth(day) ? '' : 'is-outside', iso === today ? 'is-today' : '', target ? 'is-target' : ''].filter(Boolean).join(' ')}
                    data-column={iso}
                    data-drop-cell=""
                    key={iso}
                    role="group"
                  >
                    <span className="aw-ccal__dayhead">
                      <span className="aw-ccal__date">{day.getDate()}</span>
                      {editable ? (
                        <IconButton className="aw-ccal__add" icon="plus" label={`New record on ${day.toLocaleDateString([], { month: 'long', day: 'numeric' })}`} onClick={() => void addOn(iso)} size="sm" />
                      ) : null}
                    </span>
                    {entries.slice(0, entries.length > DAY_LIMIT + 1 ? DAY_LIMIT : DAY_LIMIT + 1).map(recordLink)}
                    {entries.length > DAY_LIMIT + 1 ? <DayMore entries={entries.slice(DAY_LIMIT)} recordLink={recordLink} /> : null}
                  </div>
                )
              })}
            </div>
          ))}
        </div>
      )}
      {drag.drag && dragged ? (
        <div aria-hidden="true" className="aw-cboard__ghost" ref={ghostRef} style={{ width: drag.drag.width }}>
          <span className="aw-cboard__title">{recordName(dragged)}</span>
        </div>
      ) : null}
    </div>
  )
}

/** A day's records past the first few, behind a disclosure. */
function DayMore({ entries, recordLink }: { entries: readonly CollectionRecord[]; recordLink: (record: CollectionRecord) => ReactNode }) {
  return (
    <details className="aw-ccal__more">
      <summary>{entries.length} more</summary>
      {entries.map(recordLink)}
    </details>
  )
}

function PhoneAgenda({
  byDay,
  days,
  editable,
  onAdd,
  recordLink,
  today,
}: {
  byDay: Map<string, CollectionRecord[]>
  days: readonly Date[]
  editable: boolean
  onAdd: (day: string) => Promise<void>
  recordLink: (record: CollectionRecord) => ReactNode
  today: string
}) {
  const shown = days.filter((day) => byDay.has(toIsoDate(day)) || toIsoDate(day) === today)
  if (!shown.length) return <p className="aw-ccal__none">No records this month.</p>
  return (
    <ol className="aw-ccal__agenda">
      {shown.map((day) => {
        const iso = toIsoDate(day)
        return (
          <li className={iso === today ? 'aw-ccal__agendaday is-today' : 'aw-ccal__agendaday'} key={iso}>
            <span className="aw-ccal__dayhead">
              <span className="aw-ccal__agendadate">{day.toLocaleDateString([], { weekday: 'short', month: 'short', day: 'numeric' })}</span>
              {editable ? <IconButton icon="plus" label={`New record on ${day.toLocaleDateString([], { month: 'long', day: 'numeric' })}`} onClick={() => void onAdd(iso)} size="sm" /> : null}
            </span>
            {(byDay.get(iso) ?? []).map(recordLink)}
          </li>
        )
      })}
    </ol>
  )
}
