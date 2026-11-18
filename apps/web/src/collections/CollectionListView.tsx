import { useMemo, useRef } from 'react'

import { FieldValueView } from '../fields/FieldValueView'
import { Button, EmptyState, ListSkeleton, countLabel } from '../ui'
import { useRecords, type CollectionBody, type CollectionRecord, type ViewConfig } from './api'
import { cardValues, viewColumns } from './collectionModel'
import { NewRecordInput } from './RecordCells'
import { RecordsEmpty } from './RecordTable'
import type { RecordEdits } from './useRecordEdits'

/**
 * A collection as a list (T1.14): one line for each record, in the view's
 * order and under its groups, with the record's name and the view's shown
 * fields that have a value beside it. A line opens its record; more load
 * with a button at the end.
 */
export function CollectionListView({
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
  const records = useRecords(projectId, body.collection.id, params)
  const newRecord = useRef<HTMLInputElement>(null)
  const loaded = useMemo(() => records.data?.pages.flatMap((page) => page.records) ?? [], [records.data])
  const references = useMemo(() => records.data?.pages.flatMap((page) => page.references) ?? [], [records.data])
  const sections = useMemo(() => grouped(loaded), [loaded])
  const columns = viewColumns(body.fields, config)

  if (records.isPending) return <ListSkeleton label="Loading records" rows={6} />
  if (records.isError && !records.data) {
    return (
      <EmptyState action={<Button onClick={() => void records.refetch()}>Try again</Button>} icon="circle-alert" title="The records didn’t load">
        Check the connection and try again.
      </EmptyState>
    )
  }

  const line = (record: CollectionRecord) => (
    <li aria-current={record.id === openRecordId || undefined} className="aw-recordlist__line" key={record.id}>
      <a
        className="aw-recordlist__title"
        href={recordHref(record.id)}
        onClick={(event) => {
          if (event.metaKey || event.ctrlKey || event.shiftKey) return
          event.preventDefault()
          onOpenRecord(record.id)
        }}
      >
        <span className="aw-records__number">#{record.number}</span>
        <span className="aw-recordlist__name">{record.title || 'Untitled'}</span>
      </a>
      <span className="aw-recordlist__values">
        {cardValues(record, columns, 5).map(({ field }) => (
          <span className="aw-recordlist__value" key={field.id} title={field.name}>
            <FieldValueView field={field} references={references} value={record.values[field.id]} />
          </span>
        ))}
      </span>
    </li>
  )

  return (
    <div className="aw-recordlist">
      <p aria-live="polite" className="aw-records__summary">
        {countLabel(records.data?.pages[0]?.total ?? 0, 'record')}
      </p>
      {editable ? <NewRecordInput className="aw-recordcards__new" inputRef={newRecord} onAdd={edits.add} /> : null}
      {loaded.length === 0 ? (
        <RecordsEmpty filtered={filtered} inputRef={editable ? newRecord : null} onClearFilters={onClearFilters} />
      ) : (
        sections.map((section) => (
          <section className="aw-recordlist__section" key={section.key ?? 'all'}>
            {section.label !== null ? <h3 className="aw-recordlist__heading">{section.label}</h3> : null}
            <ul aria-label={section.label ?? body.collection.name} className="aw-recordlist__lines">
              {section.records.map(line)}
            </ul>
          </section>
        ))
      )}
      {records.hasNextPage ? (
        <Button busy={records.isFetchingNextPage} className="aw-gallery__more" onClick={() => void records.fetchNextPage()}>
          {records.isFetchNextPageError ? 'Try loading more again' : 'Show more'}
        </Button>
      ) : null}
    </div>
  )
}

/** The records in runs of one group each, as the server ordered them; one run, unlabelled, when not grouped. */
function grouped(records: readonly CollectionRecord[]) {
  const sections: { key: string | null; label: string | null; records: CollectionRecord[] }[] = []
  for (const record of records) {
    const key = record.group?.key ?? null
    const last = sections.at(-1)
    if (last && last.key === key) last.records.push(record)
    else sections.push({ key, label: record.group ? record.group.label : null, records: [record] })
  }
  return sections
}
