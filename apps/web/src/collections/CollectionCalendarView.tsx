import { useCallback, useEffect, useMemo } from 'react'
import { useSearchParams } from 'react-router-dom'

import type { FieldDefinition } from '../fields/fieldModel'
import { CalendarPiece } from '../pieces/CalendarPiece'
import { calendarRange } from '../pieces/calendarRange'
import { Banner, Button, EmptyState, countLabel } from '../ui'
import { recordsParams, useRecords, type CollectionBody, type CollectionRecord, type RecordsQuery, type ViewConfig } from './api'
import { calendarMonth, monthKey, recordDay } from './collectionModel'
import type { RecordEdits } from './useRecordEdits'

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
 * A collection as a calendar (AWD-110), drawn by the calendar piece (W1.20):
 * a month, each record on the day its date field holds. The month is in the
 * address (`?month=2026-11`). A day's + adds a record on it, and dragging a
 * record to another day changes its date.
 */
export function CollectionCalendarView(props: Props) {
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

function Calendar({ body, editable, edits, field, onOpenRecord, openRecordId, projectId, query, recordHref }: Props & { field: FieldDefinition }) {
  const [params, setParams] = useSearchParams()
  const month = calendarMonth(params.get('month'))
  const { from, to } = calendarRange(month)
  const records = useRecords(projectId, body.collection.id, recordsParams({ ...query, groupBy: null, dates: { field: field.id, from, to } }), true, 200)
  const loaded = useMemo(() => records.data?.pages.flatMap((page) => page.records) ?? [], [records.data])
  const dayOf = useCallback((record: CollectionRecord) => recordDay(record, field.id), [field.id])
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

  return (
    <CalendarPiece
      dayOf={dayOf}
      failed={records.isError && !records.data}
      href={(record) => recordHref(record.id)}
      items={loaded}
      month={month}
      notice={
        isFetchNextPageError ? (
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
        ) : null
      }
      noun="record"
      onAdd={editable ? (day) => void addOn(day) : undefined}
      onMonth={goTo}
      onMoveDay={editable ? (record, day) => void edits.save(record, { values: { [field.id]: day } }) : undefined}
      onOpen={(record) => onOpenRecord(record.id)}
      onRetry={() => void records.refetch()}
      openId={openRecordId}
      pending={records.isPending}
      summary={records.isPending ? 'Loading…' : `${countLabel(records.data?.pages[0]?.total ?? 0, 'record')} by ${field.name}`}
      title={(record) => record.title || `Untitled #${record.number}`}
    />
  )
}
