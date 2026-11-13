import { useMemo, useRef } from 'react'

import { FieldValueView } from '../fields/FieldValueView'
import { Button, EmptyState, ListSkeleton, countLabel } from '../ui'
import { useRecords, type CollectionBody, type ViewConfig } from './api'
import { cardValues, viewColumns } from './collectionModel'
import { NewRecordInput } from './RecordCells'
import { RecordsEmpty } from './RecordTable'
import type { RecordEdits } from './useRecordEdits'

/**
 * A collection as a gallery (AWD-110): a card for each record, in the view's
 * order, with the title and the view's shown fields that have a value. A
 * card opens its record; more load with a button at the end.
 */
export function RecordGallery({
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
  recordHref,
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
  recordHref: (id: string) => string
}) {
  // A gallery has no groups: the view's grouping orders a table and a board.
  const records = useRecords(projectId, body.collection.id, params)
  const newRecord = useRef<HTMLInputElement>(null)
  const loaded = useMemo(() => records.data?.pages.flatMap((page) => page.records) ?? [], [records.data])
  const references = useMemo(() => records.data?.pages.flatMap((page) => page.references) ?? [], [records.data])
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
    <div className="aw-gallery">
      <p aria-live="polite" className="aw-records__summary">
        {countLabel(records.data?.pages[0]?.total ?? 0, 'record')}
      </p>
      {editable ? <NewRecordInput className="aw-recordcards__new" inputRef={newRecord} onAdd={edits.add} /> : null}
      {loaded.length === 0 ? (
        <RecordsEmpty filtered={filtered} inputRef={editable ? newRecord : null} onClearFilters={onClearFilters} />
      ) : (
        <ul aria-label={body.collection.name} className="aw-gallery__grid">
          {loaded.map((record) => (
            <li aria-current={record.id === openRecordId || undefined} className="aw-gallery__card" key={record.id}>
              <span className="aw-gallery__head">
                <a
                  className="aw-gallery__title"
                  href={recordHref(record.id)}
                  onClick={(event) => {
                    if (event.metaKey || event.ctrlKey || event.shiftKey) return
                    event.preventDefault()
                    onOpenRecord(record.id)
                  }}
                >
                  {record.title || 'Untitled'}
                </a>
                <span className="aw-records__number">#{record.number}</span>
              </span>
              {cardValues(record, columns, 6).map(({ field }) => (
                <span className="aw-gallery__value" key={field.id}>
                  <span className="aw-gallery__name">{field.name}</span>
                  <FieldValueView field={field} references={references} value={record.values[field.id]} />
                </span>
              ))}
            </li>
          ))}
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
