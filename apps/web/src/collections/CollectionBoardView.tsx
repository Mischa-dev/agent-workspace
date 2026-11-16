import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'

import { FieldValueView } from '../fields/FieldValueView'
import type { FieldDefinition, FieldReference } from '../fields/fieldModel'
import { BoardPiece, type BoardColumn, type BoardColumnState } from '../pieces/BoardPiece'
import { Button, EmptyState, ListSkeleton, countLabel } from '../ui'
import { recordsParams, useRecords, type CollectionBody, type CollectionRecord, type RecordsQuery, type ViewConfig } from './api'
import { boardColumns, boardKey, boardValue, cardValues, recordName, viewColumns, withValues, type ShownColumn } from './collectionModel'
import type { RecordEdits } from './useRecordEdits'

/** Cards a column loads at a time. */
const COLUMN_PAGE = 50

type Props = {
  body: CollectionBody
  config: ViewConfig
  editable: boolean
  edits: RecordEdits
  filtered: boolean
  onClearFilters: () => void
  onOpenRecord: (id: string) => void
  /** Where a record opens, for its card's link. */
  recordHref: (id: string) => string
  openRecordId: string | null
  query: RecordsQuery
  projectId: string
  /** Puts the board's columns on another field, from the empty state. */
  onGroupBy: (fieldId: string) => void
  groupable: readonly FieldDefinition[]
}

/** A record on the board, with the people, tasks and pages its values name. */
type Card = { id: string; record: CollectionRecord; references: readonly FieldReference[] }

/** What each column's source needs: where its records come from, and the board's query. */
type Source = { projectId: string; collectionId: string; query: RecordsQuery; field: FieldDefinition }

/**
 * A collection as a board (AWD-110), drawn by the board piece (W1.19): a
 * column for each value of the field the view groups by, each loading its own
 * records in the view's order. Moving a card changes that value; the card
 * shows in its new column at once and saves like any other change.
 */
export function CollectionBoardView(props: Props) {
  // A grouping a card can't move between (a date, several choices) isn't a board's.
  const field = props.groupable.find((entry) => entry.id === props.config.groupBy)
  if (!field) {
    return (
      <EmptyState icon="board" title="Pick what the columns are">
        {props.groupable.length ? (
          <>
            A board has a column for each value of a choice, status, person or checkbox field.
            <span className="aw-cboard__pick">
              {props.groupable.map((entry) => (
                <Button key={entry.id} onClick={() => props.onGroupBy(entry.id)} size="sm">
                  {entry.name}
                </Button>
              ))}
            </span>
          </>
        ) : (
          'A board has a column for each value of a choice, status, person or checkbox field. Add one to this collection first.'
        )}
      </EmptyState>
    )
  }
  return <Board {...props} field={field} />
}

function Board({ body, config, editable, edits, field, filtered, onClearFilters, onOpenRecord, openRecordId, projectId, query, recordHref }: Props & { field: FieldDefinition }) {
  const collectionId = body.collection.id
  // The view's own query, one record long: its total and each group's count.
  const counts = useRecords(projectId, collectionId, recordsParams({ ...query, groupBy: field.id }), true, 1)
  const first = counts.data?.pages[0]
  const columns = useMemo(() => boardColumns(field, first?.groups ?? []), [field, first?.groups])
  const shown = viewColumns(body.fields, config).filter((column) => column.field.id !== field.id)
  const source = useMemo<Source>(() => ({ projectId, collectionId, query, field }), [collectionId, field, projectId, query])
  const columnOf = useCallback((card: Card) => boardKey(card.record, field, columns), [columns, field])
  const movedTo = useCallback(
    (card: Card, key: string): Card => ({ ...card, record: { ...card.record, values: withValues(card.record.values, { [field.id]: boardValue(field, key) }) } }),
    [field],
  )
  // A failed save goes back where it was (the save says why); the lists refetch.
  const move = useCallback((card: Card, key: string) => edits.save(card.record, { values: { [field.id]: boardValue(field, key) } }).then((saved) => saved === true), [edits, field])

  if (counts.isPending) return <ListSkeleton label="Loading the board" rows={4} />
  if (counts.isError && !counts.data) {
    return (
      <EmptyState action={<Button onClick={() => void counts.refetch()}>Try again</Button>} icon="circle-alert" title="The board didn’t load">
        Check the connection and try again.
      </EmptyState>
    )
  }

  return (
    <BoardPiece
      // A card moved while the query changes (a search, a filter) doesn't stay on the new board.
      key={JSON.stringify(source.query)}
      Source={RecordColumn}
      badge={(card) => <span className="aw-records__number">#{card.record.number}</span>}
      columnOf={columnOf}
      columns={columns}
      details={(card) => <CardValues card={card} shown={shown} />}
      href={(card) => recordHref(card.id)}
      label={`${body.collection.name}, by ${field.name}`}
      movedTo={movedTo}
      movesBy={field.name}
      onMove={editable ? move : undefined}
      onOpen={(card) => onOpenRecord(card.id)}
      openId={openRecordId}
      source={source}
      summary={
        <p aria-live="polite" className="aw-records__summary">
          {countLabel(first?.total ?? 0, 'record')}
          {filtered && !first?.total ? (
            <>
              {' '}
              <Button onClick={onClearFilters} size="sm" variant="ghost">
                Clear filters
              </Button>
            </>
          ) : null}
        </p>
      }
      title={(card) => recordName(card.record)}
    />
  )
}

/** One column's records, a page at a time, in the view's order. */
function RecordColumn({ children, column, source }: { column: BoardColumn; source: Source; children: (state: BoardColumnState<Card>) => ReactNode }) {
  const records = useRecords(source.projectId, source.collectionId, recordsParams({ ...source.query, groupBy: source.field.id, group: column.key }), true, COLUMN_PAGE)
  const items = useMemo(
    () => records.data?.pages.flatMap((page) => page.records.map((record) => ({ id: record.id, record, references: page.references }))) ?? [],
    [records.data],
  )
  // When the newest fetch from the server that worked began. Patches to the
  // cache (an edit elsewhere) aren't fetches, so they don't count.
  const [fetchedFrom, setFetchedFrom] = useState(0)
  const fetchStarted = useRef<number | null>(null)
  const fetching = records.isFetching
  const failed = records.isError
  useEffect(() => {
    if (fetching) {
      fetchStarted.current ??= Date.now()
      return
    }
    const started = fetchStarted.current
    fetchStarted.current = null
    if (started !== null && !failed) setFetchedFrom(started)
  }, [failed, fetching])
  return children({
    items,
    pending: records.isPending,
    failed: records.isError && !records.data,
    onRetry: () => void records.refetch(),
    hasMore: records.hasNextPage,
    loadingMore: records.isFetchingNextPage,
    onLoadMore: () => void records.fetchNextPage(),
    fetchedFrom,
  })
}

function CardValues({ card, shown }: { card: Card; shown: readonly ShownColumn[] }) {
  return cardValues(card.record, shown, 3).map(({ field: value }) => (
    <span className="aw-cboard__value" key={value.id}>
      <span className="aw-cboard__name">{value.name}</span>
      <FieldValueView field={value} references={card.references} value={card.record.values[value.id]} />
    </span>
  ))
}
