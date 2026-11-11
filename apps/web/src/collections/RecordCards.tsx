import { useMemo, useRef } from 'react'

import { FieldValueView } from '../fields/FieldValueView'
import { displayText } from '../fields/fieldModel'
import { Button, EmptyState, ListSkeleton, countLabel } from '../ui'
import type { CollectionBody, ViewConfig } from './api'
import { useRecords } from './api'
import { cardValues, tableItems, viewColumns } from './collectionModel'
import { NewRecordInput } from './RecordCells'
import { RecordsEmpty } from './RecordTable'
import type { RecordEdits } from './useRecordEdits'

/**
 * A collection's records on a phone, where a wide table doesn't fit: a card
 * each with the title, number and the first values that are set, and a tap
 * opens the whole record. A grouped view heads each group; more load with a
 * button at the end.
 */
export function RecordCards({
  body,
  config,
  editable,
  edits,
  filtered,
  onClearFilters,
  onOpenRecord,
  openRecordId,
  params,
  projectId,
}: {
  body: CollectionBody
  config: ViewConfig
  editable: boolean
  edits: RecordEdits
  filtered: boolean
  onClearFilters: () => void
  onOpenRecord: (id: string) => void
  openRecordId: string | null
  params: string
  projectId: string
}) {
  const records = useRecords(projectId, body.collection.id, params)
  const newRecord = useRef<HTMLInputElement>(null)
  const loaded = useMemo(() => records.data?.pages.flatMap((page) => page.records) ?? [], [records.data])
  const references = useMemo(() => records.data?.pages.flatMap((page) => page.references) ?? [], [records.data])
  const first = records.data?.pages[0]
  const items = useMemo(() => tableItems(loaded, first?.groups ?? [], Boolean(config.groupBy), new Set(), false), [config.groupBy, first?.groups, loaded])
  const columns = viewColumns(body.fields, config)

  if (records.isPending) return <ListSkeleton label="Loading records" rows={4} />
  if (records.isError && !records.data) {
    return (
      <EmptyState action={<Button onClick={() => void records.refetch()}>Try again</Button>} icon="circle-alert" title="The records didn’t load">
        Check the connection and try again.
      </EmptyState>
    )
  }

  return (
    <div className="aw-recordcards">
      <p aria-live="polite" className="aw-records__summary">
        {countLabel(first?.total ?? 0, 'record')}
      </p>
      {editable ? <NewRecordInput className="aw-recordcards__new" inputRef={newRecord} onAdd={edits.add} /> : null}
      {loaded.length === 0 ? (
        <RecordsEmpty filtered={filtered} inputRef={editable ? newRecord : null} onClearFilters={onClearFilters} />
      ) : (
        <ul aria-label={body.collection.name} className="aw-recordcards__list">
          {items.map((item) => {
            if (item.kind === 'group') {
              return (
                <li className="aw-recordcards__group" key={`group:${item.key}`}>
                  {item.color ? <span aria-hidden className="aw-label__dot" style={{ backgroundColor: item.color }} /> : null}
                  <span className="aw-records__grouplabel">{item.label || 'None'}</span>
                  <span className="aw-records__groupcount">{item.count}</span>
                </li>
              )
            }
            if (item.kind !== 'record') return null
            const { record } = item
            return (
              <li key={record.id}>
                <button
                  aria-current={record.id === openRecordId || undefined}
                  className="aw-recordcards__card"
                  onClick={() => onOpenRecord(record.id)}
                  type="button"
                >
                  <span className="aw-recordcards__head">
                    <span className="aw-recordcards__title">{record.title || 'Untitled'}</span>
                    <span className="aw-records__number">#{record.number}</span>
                  </span>
                  {cardValues(record, columns).map(({ field }) => (
                    <span className="aw-recordcards__value" key={field.id}>
                      <span className="aw-recordcards__name">{field.name}</span>
                      {/* A web address reads as text here: a link can't sit inside a button. */}
                      {field.type === 'url' ? (
                        <span className="aw-prop__text">{displayText(field, record.values[field.id], references)}</span>
                      ) : (
                        <FieldValueView field={field} references={references} value={record.values[field.id]} />
                      )}
                    </span>
                  ))}
                </button>
              </li>
            )
          })}
        </ul>
      )}
      {records.hasNextPage ? (
        <Button busy={records.isFetchingNextPage} className="aw-recordcards__more" onClick={() => void records.fetchNextPage()}>
          {records.isFetchNextPageError ? 'Try loading more again' : 'Show more'}
        </Button>
      ) : null}
    </div>
  )
}
