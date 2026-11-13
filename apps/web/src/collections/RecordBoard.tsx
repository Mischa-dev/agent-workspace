import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react'

import { FieldValueView } from '../fields/FieldValueView'
import type { FieldDefinition, FieldReference } from '../fields/fieldModel'
import { Button, EmptyState, IconButton, ListSkeleton, Menu, countLabel, type MenuEntry } from '../ui'
import { useBoardDrag, type DropTarget } from '../work-items/useBoardDrag'
import { recordsParams, useRecords, type CollectionBody, type CollectionRecord, type RecordsQuery, type ViewConfig } from './api'
import { boardColumns, boardKey, boardValue, cardValues, recordName, viewColumns, withValues, type BoardColumn, type ShownColumn } from './collectionModel'
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

/** A card moved to another column, shown there until the column loads it. */
type Moved = { record: CollectionRecord; key: string; at: number }

/**
 * A collection as a board (AWD-110): a column for each value of the field
 * the view groups by, each loading its own records in the view's order.
 * Dragging a card to another column, or its menu's Move to, changes that
 * value; the card shows there at once and saves like any other change.
 */
export function RecordBoard(props: Props) {
  const field = props.body.fields.find((entry) => entry.id === props.config.groupBy && !entry.archived)
  if (!field) {
    return (
      <EmptyState icon="board" title="Pick what the columns are">
        {props.groupable.length ? (
          <>
            A board has a column for each value of a choice, person or checkbox field.
            <span className="aw-cboard__pick">
              {props.groupable.map((entry) => (
                <Button key={entry.id} onClick={() => props.onGroupBy(entry.id)} size="sm">
                  {entry.name}
                </Button>
              ))}
            </span>
          </>
        ) : (
          'A board has a column for each value of a choice, person or checkbox field. Add one to this collection first.'
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
  const [moved, setMoved] = useState<Moved[]>([])
  const ghostRef = useRef<HTMLDivElement>(null)
  const scrollerRef = useRef<HTMLDivElement>(null)
  // Each column's cards as it shows them, to find the one a drop names.
  const [shownCards, setShownCards] = useState<Record<string, readonly CollectionRecord[]>>({})
  const onCards = useCallback((key: string, cards: readonly CollectionRecord[]) => setShownCards((current) => (current[key] === cards ? current : { ...current, [key]: cards })), [])
  const findCard = (id: string) => Object.values(shownCards).flat().find((record) => record.id === id)
  // Moves the next refetch has already counted are in their column's list by then.
  const countedAt = counts.dataUpdatedAt
  const pending = useMemo(() => moved.filter((entry) => entry.at > countedAt), [countedAt, moved])

  const move = (record: CollectionRecord, key: string) => {
    if (boardKey(record, field, columns) === key) return
    const value = boardValue(field, key)
    setMoved((current) => [...current.filter((entry) => entry.record.id !== record.id), { record: { ...record, values: withValues(record.values, { [field.id]: value }) }, key, at: Date.now() }])
    void edits.save(record, { values: { [field.id]: value } })
  }
  const drag = useBoardDrag({
    enabled: editable,
    ghostRef,
    scrollerRef,
    onDrop: (id: string, target: DropTarget) => {
      const record = findCard(id)
      if (record) move(record, target.columnKey)
    },
  })
  const dragged = drag.drag ? findCard(drag.drag.cardId) : undefined

  if (counts.isPending) return <ListSkeleton label="Loading the board" rows={4} />
  if (counts.isError && !counts.data) {
    return (
      <EmptyState action={<Button onClick={() => void counts.refetch()}>Try again</Button>} icon="circle-alert" title="The board didn’t load">
        Check the connection and try again.
      </EmptyState>
    )
  }

  return (
    <div className="aw-cboard-wrap">
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
      <div className={drag.drag ? 'aw-cboard is-dragging' : 'aw-cboard'} ref={scrollerRef} style={{ ['--aw-board-columns' as string]: columns.length }}>
        {columns.map((column) => (
          <Column
            column={column}
            columns={columns}
            editable={editable}
            field={field}
            key={column.key}
            moved={pending}
            onMove={move}
            onOpenRecord={(id) => {
              if (!drag.justDragged()) onOpenRecord(id)
            }}
            onPointerDown={drag.onPointerDown}
            onCards={onCards}
            openRecordId={openRecordId}
            params={recordsParams({ ...query, groupBy: field.id, group: column.key })}
            projectId={projectId}
            collectionId={collectionId}
            recordHref={recordHref}
            shown={shown}
            target={drag.drag?.target?.columnKey === column.key}
          />
        ))}
      </div>
      {drag.drag && dragged ? (
        <div aria-hidden="true" className="aw-cboard__ghost" ref={ghostRef} style={{ width: drag.drag.width }}>
          <span className="aw-cboard__title">{recordName(dragged)}</span>
        </div>
      ) : null}
    </div>
  )
}

function Column({
  collectionId,
  column,
  columns,
  editable,
  field,
  moved,
  onMove,
  onOpenRecord,
  onPointerDown,
  onCards,
  openRecordId,
  params,
  projectId,
  recordHref,
  shown,
  target,
}: {
  collectionId: string
  column: BoardColumn
  columns: readonly BoardColumn[]
  editable: boolean
  field: FieldDefinition
  moved: readonly Moved[]
  onMove: (record: CollectionRecord, key: string) => void
  onOpenRecord: (id: string) => void
  onPointerDown: (event: ReactPointerEvent<HTMLElement>, id: string) => void
  onCards: (key: string, records: readonly CollectionRecord[]) => void
  openRecordId: string | null
  params: string
  projectId: string
  recordHref: (id: string) => string
  shown: readonly ShownColumn[]
  target: boolean
}) {
  const records = useRecords(projectId, collectionId, params, true, COLUMN_PAGE)
  const loaded = useMemo(() => records.data?.pages.flatMap((page) => page.records) ?? [], [records.data])
  const references = useMemo(() => records.data?.pages.flatMap((page) => page.references) ?? [], [records.data])
  // A card stays in the column its value says now: one moved away leaves at once, one moved here joins.
  const cards = useMemo(() => {
    const here = loaded.filter((record) => boardKey(record, field, columns) === column.key)
    const joined = moved.filter((entry) => entry.key === column.key && !here.some((record) => record.id === entry.record.id)).map((entry) => entry.record)
    return [...here, ...joined]
  }, [column.key, columns, field, loaded, moved])
  useEffect(() => onCards(column.key, cards), [cards, column.key, onCards])
  const label = column.label || 'No value'

  return (
    <section aria-label={label} className={target ? 'aw-cboard__column is-target' : 'aw-cboard__column'} data-column={column.key} data-drop-cell="">
      <header className="aw-cboard__head">
        {column.color ? <span aria-hidden className="aw-label__dot" style={{ backgroundColor: column.color }} /> : null}
        <span className="aw-cboard__label">{label}</span>
        <span className="aw-records__groupcount">{column.count}</span>
      </header>
      {records.isPending ? (
        <ListSkeleton label={`Loading ${label}`} rows={2} />
      ) : (
        <ol className="aw-cboard__cards">
          {cards.map((record) => (
            <Card
              columns={columns}
              editable={editable}
              field={field}
              key={record.id}
              onMove={onMove}
              onOpen={onOpenRecord}
              onPointerDown={onPointerDown}
              open={record.id === openRecordId}
              record={record}
              recordHref={recordHref}
              references={references}
              shown={shown}
            />
          ))}
        </ol>
      )}
      {records.isError && !records.data ? (
        <Button onClick={() => void records.refetch()} size="sm" variant="ghost">
          Try loading again
        </Button>
      ) : null}
      {records.hasNextPage ? (
        <Button busy={records.isFetchingNextPage} onClick={() => void records.fetchNextPage()} size="sm" variant="ghost">
          Show more ({Math.max(0, column.count - loaded.length)})
        </Button>
      ) : null}
    </section>
  )
}

function Card({
  columns,
  editable,
  field,
  onMove,
  onOpen,
  onPointerDown,
  open,
  record,
  recordHref,
  references,
  shown,
}: {
  columns: readonly BoardColumn[]
  editable: boolean
  field: FieldDefinition
  onMove: (record: CollectionRecord, key: string) => void
  onOpen: (id: string) => void
  onPointerDown: (event: ReactPointerEvent<HTMLElement>, id: string) => void
  open: boolean
  record: CollectionRecord
  recordHref: (id: string) => string
  references: readonly FieldReference[]
  shown: readonly ShownColumn[]
}) {
  const name = recordName(record)
  const here = boardKey(record, field, columns)
  const moveItems: MenuEntry[] = [
    { heading: `Move to (${field.name})` },
    ...columns.map((column) => ({ label: column.label || 'No value', checked: column.key === here, onSelect: () => onMove(record, column.key) })),
  ]
  return (
    <li
      aria-current={open || undefined}
      className={editable ? 'aw-cboard__card is-movable' : 'aw-cboard__card'}
      data-drag-id={record.id}
      onPointerDown={editable ? (event) => onPointerDown(event, record.id) : undefined}
    >
      <span className="aw-cboard__cardhead">
        <a
          className="aw-cboard__title"
          href={recordHref(record.id)}
          onClick={(event) => {
            if (event.metaKey || event.ctrlKey || event.shiftKey) return
            event.preventDefault()
            onOpen(record.id)
          }}
        >
          {record.title || 'Untitled'}
        </a>
        <span className="aw-records__number">#{record.number}</span>
        {editable ? <Menu items={moveItems} label={`Move ${name}`} placement="bottom-end" trigger={<IconButton className="aw-cboard__menu" icon="more" label={`Move ${name}`} size="sm" />} /> : null}
      </span>
      {cardValues(record, shown, 3).map(({ field: value }) => (
        <span className="aw-cboard__value" key={value.id}>
          <span className="aw-cboard__name">{value.name}</span>
          <FieldValueView field={value} references={references} value={record.values[value.id]} />
        </span>
      ))}
    </li>
  )
}
