import { useMemo, useRef, useState, type ReactNode } from 'react'

import { FieldEditor } from '../fields/FieldEditor'
import type { FieldDefinition, FieldValue } from '../fields/fieldModel'
import { TablePiece, type TablePieceItem } from '../pieces/TablePiece'
import { Button, EmptyState, Icon, ListSkeleton, countLabel } from '../ui'
import type { TableColumn } from '../ui/Table'
import type { CollectionBody, CollectionRecord, RecordPage, ViewConfig } from './api'
import { BUILT_IN_TASK_FIELDS, useRecords } from './api'
import { TITLE_WIDTH, referenceKind, tableItems, viewColumns } from './collectionModel'
import { AddFieldHeader, FieldHeader, TitleHeader, type ColumnActions } from './ColumnMenus'
import { NewRecordInput, TitleCell, ValueCell } from './RecordCells'
import { RecordsEmpty } from './RecordsEmpty'
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
 * A collection's records as a table you edit in place, drawn by the table
 * piece (W1.18): the view's columns, a click on a value opens its editor, a
 * title renames in place, a group header folds its records, and the last row
 * adds a record. The piece draws a screenful at a time and asks for the next
 * page as the table scrolls.
 */
export function CollectionTableView(props: Props) {
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
  const [collapsedState, setCollapsedState] = useState<{ groupBy: string | null; keys: Set<string> }>({ groupBy: null, keys: new Set() })
  const collapsed = useMemo(() => (collapsedState.groupBy === config.groupBy ? collapsedState.keys : new Set<string>()), [collapsedState, config.groupBy])
  const [editing, setEditing] = useState<Editing | null>(null)
  const picked = useRef<string | null>(null)
  const newRecord = useRef<HTMLInputElement>(null)

  const loaded = useMemo(() => records.data.pages.flatMap((page) => page.records), [records.data])
  const references = useMemo(() => records.data.pages.flatMap((page) => page.references), [records.data])
  const first = records.data.pages[0]
  const items = useMemo<TablePieceItem<CollectionRecord>[]>(
    () =>
      tableItems(loaded, first?.groups ?? [], Boolean(config.groupBy), collapsed, Boolean(records.hasNextPage)).map((item) =>
        item.kind === 'record'
          ? // A record grouped by several choices is a row in each of its groups.
            { kind: 'item', key: `${item.record.group?.key ?? ''}:${item.record.id}`, item: item.record }
          : item,
      ),
    [collapsed, config.groupBy, first?.groups, loaded, records.hasNextPage],
  )
  const columns = viewColumns(body.fields, config)

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
      header: <TitleHeader actions={actions} canBuild={body.canBuild && !body.collection.builtin} config={config} fields={body.fields} titleName={body.collection.titleName} />,
      width: `${config.titleWidth ?? TITLE_WIDTH}px`,
    },
    ...columns.map(({ field, width }) => ({
      id: field.id,
      header: <FieldHeader actions={actions} canBuild={body.canBuild && !(body.collection.actsLikeTask && BUILT_IN_TASK_FIELDS.has(field.id))} config={config} field={field} fields={body.fields} titleName={body.collection.titleName} />,
      width: `${width}px`,
    })),
    { id: 'add', header: body.canBuild ? <AddFieldHeader actions={actions} /> : null, width: 'minmax(48px, 1fr)' },
  ]

  const cells = (record: CollectionRecord): Record<string, ReactNode> => ({
    number: <span className="aw-records__number">#{record.number}</span>,
    title: <TitleCell editable={editable} onOpen={() => onOpenRecord(record.id)} onRename={(title) => void edits.save(record, { title })} record={record} />,
    ...Object.fromEntries(
      columns.map(({ field }) => [
        field.id,
        <ValueCell
          collection={{ projectId, id: body.collection.id }}
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

  const { fetchNextPage } = records
  return (
    <>
      <TablePiece
        cells={cells}
        columns={tableColumns}
        empty={loaded.length === 0 ? <RecordsEmpty filtered={filtered} inputRef={editable ? newRecord : null} onClearFilters={onClearFilters} /> : null}
        footer={editable ? <NewRecordInput className="aw-records__new" inputRef={newRecord} onAdd={edits.add} /> : null}
        hasMore={Boolean(records.hasNextPage)}
        isOpen={(record) => record.id === openRecordId}
        items={items}
        label={body.collection.name}
        loadFailed={records.isFetchNextPageError}
        loadingMore={records.isFetchingNextPage}
        onLoadMore={() => void fetchNextPage()}
        onOpen={(record) => onOpenRecord(record.id)}
        onToggleGroup={toggleGroup}
        rowLabel={(record) => `#${record.number} ${record.title}`}
        summary={
          <>
            {countLabel(first?.total ?? 0, 'record')}
            {records.isFetching && !records.isFetchingNextPage ? <Icon className="aw-spin" name="loader" size={12} /> : null}
          </>
        }
      />
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
          recordId={editing.record.id}
          references={references}
          value={editing.record.values[editing.field.id]}
        />
      ) : null}
    </>
  )
}
