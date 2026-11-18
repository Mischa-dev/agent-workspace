import { useMemo, useRef } from 'react'

import { FieldValueView } from '../fields/FieldValueView'
import { GalleryPiece } from '../pieces/GalleryPiece'
import { Button, EmptyState, ListSkeleton, countLabel } from '../ui'
import { useRecords, type CollectionBody, type ViewConfig } from './api'
import { cardValues, viewColumns } from './collectionModel'
import { NewRecordInput } from './RecordCells'
import { RecordsEmpty } from './RecordsEmpty'
import type { RecordEdits } from './useRecordEdits'

/**
 * A collection as a gallery (AWD-110), drawn by the gallery piece (W1.20): a
 * card for each record, in the view's order, with the title and the view's
 * shown fields that have a value. A card opens its record; more load with a
 * button at the end.
 */
export function CollectionGalleryView({
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
    <GalleryPiece
      badge={(record) => <span className="aw-records__number">#{record.number}</span>}
      details={(record) =>
        cardValues(record, columns, 6).map(({ field }) => (
          <span className="aw-gallery__value" key={field.id}>
            <span className="aw-gallery__name">{field.name}</span>
            <FieldValueView field={field} references={references} value={record.values[field.id]} />
          </span>
        ))
      }
      empty={<RecordsEmpty filtered={filtered} inputRef={editable ? newRecord : null} onClearFilters={onClearFilters} />}
      hasMore={records.hasNextPage}
      header={
        <>
          <p aria-live="polite" className="aw-records__summary">
            {countLabel(records.data?.pages[0]?.total ?? 0, 'record')}
          </p>
          {editable ? <NewRecordInput className="aw-recordcards__new" inputRef={newRecord} onAdd={edits.add} /> : null}
        </>
      }
      href={(record) => recordHref(record.id)}
      items={loaded}
      label={body.collection.name}
      loadingMore={records.isFetchingNextPage}
      moreFailed={records.isFetchNextPageError}
      onLoadMore={() => void records.fetchNextPage()}
      onOpen={(record) => onOpenRecord(record.id)}
      openId={openRecordId}
      title={(record) => record.title || 'Untitled'}
    />
  )
}
