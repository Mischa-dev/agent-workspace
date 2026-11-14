import { useEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from 'react'

import { FieldEditor } from '../fields/FieldEditor'
import type { FieldDefinition, FieldValue } from '../fields/fieldModel'
import { Button, EmptyState, Icon, ListSkeleton, countLabel } from '../ui'
import { Table, type TableColumn, type TableRow } from '../ui/pickers'
import { useCoarsePointer } from '../ui/useCoarsePointer'
import { useWindowedRows } from '../ui/useWindowedRows'
import type { CollectionBody, CollectionRecord, RecordPage, ViewConfig } from './api'
import { useRecords } from './api'
import { TITLE_WIDTH, referenceKind, tableItems, viewColumns, type TableItem } from './collectionModel'
import { AddFieldHeader, FieldHeader, TitleHeader, type ColumnActions } from './ColumnMenus'
import { NewRecordInput, TitleCell, ValueCell } from './RecordCells'
import type { RecordEdits } from './useRecordEdits'
import '../styles/work.css'

type Editing = { record: CollectionRecord; field: FieldDefinition; anchor: HTMLElement }

type Props = {
  actions: ColumnActions
  body: CollectionBody
  config: ViewConfig
  /** Values can be changed here: the person may, and nothing is archived. */
  editable: boolean
  edits: RecordEdits
  /** Whether filters or a search are narrowing the list, for the empty state. */
  filtered: boolean
  onClearFilters: () => void
  onOpenRecord: (id: string) => void
  openRecordId: string | null
  params: string
  projectId: string
}

/**
 * A collection's records as a table you edit in place: the view's columns,
 * a click on a value opens its editor, a title renames in place, a group
 * header folds its records, and the last row adds a record. Rows are drawn a
 * screenful at a time and load page by page as the table scrolls.
 */
export function RecordTable(props: Props) {
  const records = useRecords(props.projectId, props.body.collection.id, props.params)
  if (records.isPending) return <ListSkeleton label="Loading records" rows={6} />
  if (records.isError && !records.data) {
    return (
      <EmptyState action={<Button onClick={() => void records.refetch()}>Try again</Button>} icon="circle-alert" title="The records didn’t load">
        Check the connection and try again.
      </EmptyState>
    )
  }
  return <RecordGrid {...props} records={records} />
}

type Loaded = ReturnType<typeof useRecords> & { data: { pages: RecordPage[] } }

/** The table once its first page is in, so windowing finds the box it scrolls in. */
function RecordGrid({
  actions,
  body,
  config,
  editable,
  edits,
  filtered,
  onClearFilters,
  onOpenRecord,
  openRecordId,
  projectId,
  records,
}: Props & { records: Loaded }) {
  const coarse = useCoarsePointer()
  const rowHeight = coarse ? 44 : 36
  const [collapsedState, setCollapsedState] = useState<{ groupBy: string | null; keys: Set<string> }>({ groupBy: null, keys: new Set() })
  const collapsed = useMemo(() => (collapsedState.groupBy === config.groupBy ? collapsedState.keys : new Set<string>()), [collapsedState, config.groupBy])
  const [editing, setEditing] = useState<Editing | null>(null)
  const picked = useRef<string | null>(null)
  const newRecord = useRef<HTMLInputElement>(null)
  const bodyRef = useRef<HTMLDivElement>(null)

  const loaded = useMemo(() => records.data.pages.flatMap((page) => page.records), [records.data])
  const references = useMemo(() => records.data.pages.flatMap((page) => page.references), [records.data])
  const first = records.data.pages[0]
  const items = useMemo(
    () => tableItems(loaded, first?.groups ?? [], Boolean(config.groupBy), collapsed, Boolean(records.hasNextPage)),
    [collapsed, config.groupBy, first?.groups, loaded, records.hasNextPage],
  )
  const rows = useMemo<(TableItem | { kind: 'new' })[]>(() => (editable ? [...items, { kind: 'new' }] : items), [editable, items])
  const heights = useMemo(() => rows.map(() => rowHeight), [rowHeight, rows])
  const windowed = useWindowedRows(bodyRef, heights)
  const columns = viewColumns(body.fields, config)

  // The next page loads when its placeholder row scrolls into view.
  const loadingVisible = rows.slice(windowed.start, windowed.end).some((row) => row.kind === 'loading')
  const { fetchNextPage, hasNextPage, isFetchingNextPage, isFetchNextPageError } = records
  useEffect(() => {
    if (loadingVisible && hasNextPage && !isFetchingNextPage && !isFetchNextPageError) void fetchNextPage()
  }, [fetchNextPage, hasNextPage, isFetchNextPageError, isFetchingNextPage, loadingVisible])

  const toggleGroup = (key: string) => {
    const keys = new Set(collapsed)
    if (keys.has(key)) keys.delete(key)
    else keys.add(key)
    setCollapsedState({ groupBy: config.groupBy, keys })
  }

  const tableColumns: TableColumn[] = [
    { id: 'number', header: '#', width: '64px' },
    {
      id: 'title',
      header: <TitleHeader actions={actions} canBuild={body.canBuild} config={config} fields={body.fields} titleName={body.collection.titleName} />,
      width: `${config.titleWidth ?? TITLE_WIDTH}px`,
    },
    ...columns.map(({ field, width }) => ({
      id: field.id,
      header: <FieldHeader actions={actions} canBuild={body.canBuild} config={config} field={field} fields={body.fields} titleName={body.collection.titleName} />,
      width: `${width}px`,
    })),
    { id: 'add', header: body.canBuild ? <AddFieldHeader actions={actions} /> : null, width: 'minmax(48px, 1fr)' },
  ]

  const recordCells = (record: CollectionRecord): Record<string, ReactNode> => ({
    number: <span className="aw-records__number">#{record.number}</span>,
    title: <TitleCell editable={editable} onOpen={() => onOpenRecord(record.id)} onRename={(title) => void edits.save(record, { title })} record={record} />,
    ...Object.fromEntries(
      columns.map(({ field }) => [
        field.id,
        <ValueCell
          editable={editable}
          field={field}
          key={field.id}
          onEdit={(anchor) => setEditing({ record, field, anchor })}
          onToggle={() => void edits.save(record, { values: { [field.id]: record.values[field.id] === true ? null : true } })}
          record={record}
          references={references}
        />,
      ]),
    ),
  })

  const toRow = (row: TableItem | { kind: 'new' }, index: number): TableRow => {
    if (row.kind === 'loading') {
      return {
        key: 'loading',
        index,
        inert: true,
        height: rowHeight,
        full: isFetchNextPageError ? (
          <span className="aw-records__loading">
            More records didn’t load.
            <Button onClick={() => void fetchNextPage()} size="sm">
              Try again
            </Button>
          </span>
        ) : (
          <span className="aw-records__loading" role="status">
            <Icon className="aw-spin" name="loader" size={14} /> Loading more…
          </span>
        ),
      }
    }
    if (row.kind === 'new') {
      return { key: 'new', index, inert: true, height: rowHeight, full: <NewRecordInput className="aw-records__new" inputRef={newRecord} onAdd={edits.add} /> }
    }
    if (row.kind === 'group') {
      return {
        key: `group:${row.key}`,
        index,
        height: rowHeight,
        className: 'aw-records__grouprow',
        label: `${row.label || 'None'}, ${countLabel(row.count, 'record')}, ${row.collapsed ? 'collapsed' : 'expanded'}`,
        full: (
          <button aria-expanded={!row.collapsed} className="aw-records__group" onClick={() => toggleGroup(row.key)} tabIndex={-1} type="button">
            <Icon name={row.collapsed ? 'chevron-right' : 'chevron-down'} size={14} />
            {row.color ? <span aria-hidden className="aw-label__dot" style={{ backgroundColor: row.color }} /> : null}
            <span className="aw-records__grouplabel">{row.label || 'None'}</span>
            <span className="aw-records__groupcount">{row.count}</span>
          </button>
        ),
      }
    }
    const record = row.record
    return {
      key: record.id,
      index,
      height: rowHeight,
      className: record.id === openRecordId ? 'is-open' : undefined,
      label: `#${record.number} ${record.title}`,
      cells: recordCells(record),
    }
  }

  const tableRows = rows.slice(windowed.start, windowed.end).map((row, offset) => toRow(row, windowed.start + offset))
  const total = first?.total ?? 0
  const open = (key: string) => {
    if (key.startsWith('group:')) toggleGroup(key.slice('group:'.length))
    else onOpenRecord(key)
  }

  return (
    <div className="aw-records">
      <p aria-live="polite" className="aw-records__summary">
        {countLabel(total, 'record')}
        {records.isFetching && !records.isFetchingNextPage ? <Icon className="aw-spin" name="loader" size={12} /> : null}
      </p>
      <div className="aw-records__scroll">
        <Table
          activeKey={null}
          bodyRef={bodyRef}
          className="aw-records__table"
          columns={tableColumns}
          label={body.collection.name}
          onActivate={open}
          onRowClick={(_event, key) => open(key)}
          rowCount={rows.length}
          rows={tableRows}
          spaceAfter={windowed.after}
          spaceBefore={windowed.before}
        />
        {loaded.length === 0 ? (
          <RecordsEmpty filtered={filtered} inputRef={editable ? newRecord : null} onClearFilters={onClearFilters} />
        ) : null}
      </div>
      {editing ? (
        <FieldEditor
          anchor={editing.anchor}
          field={editing.field}
          onClose={() => setEditing(null)}
          onLabel={(label) => {
            picked.current = label
          }}
          onSet={(value: FieldValue | null) => {
            const { record, field } = editing
            const label = picked.current
            picked.current = null
            const reference = label && typeof value === 'string' ? { id: value, kind: referenceKind(field), label, key: null } : undefined
            void edits.save(record, { values: { [field.id]: value } }, reference)
          }}
          projectId={projectId}
          references={references}
          value={editing.record.values[editing.field.id]}
        />
      ) : null}
    </div>
  )
}

/** No records to show: none yet, or none that match. */
export function RecordsEmpty({
  filtered,
  inputRef,
  onClearFilters,
}: {
  filtered: boolean
  inputRef: RefObject<HTMLInputElement | null> | null
  onClearFilters: () => void
}) {
  return (
    <EmptyState
      action={
        filtered ? (
          <Button onClick={onClearFilters}>Clear filters</Button>
        ) : inputRef ? (
          <Button icon="plus" onClick={() => inputRef.current?.focus()}>
            New record
          </Button>
        ) : null
      }
      icon="table"
      title={filtered ? 'No matching records' : 'No records yet'}
    >
      {filtered ? 'Try a different search, or clear the filters.' : 'Each record is a row: give it a name, then fill in its fields here or from an agent.'}
    </EmptyState>
  )
}
