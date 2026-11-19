import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'

import { Button, Icon, countLabel } from '../ui'
import { Table, type TableColumn, type TableRow } from '../ui/Table'
import { useCoarsePointer } from '../ui/useCoarsePointer'
import { useWindowedRows } from '../ui/useWindowedRows'
import './table.css'

// The table piece (W1.18): rows of things under a sticky header, a
// screenful drawn at a time however many there are, group headers that fold
// their rows, a row that loads the next page as it scrolls into view, and a
// last row the owner fills (adding one, say). The keyboard moves between rows
// (the kit table) and Enter opens one. What the cells hold, and how a value
// is edited in place, are the owner's: it passes the cells and handles
// their changes (collections today, tasks with T1).

export type TablePieceGroup = { kind: 'group'; key: string; label: string; count: number; collapsed: boolean; color?: string | null }

/** One row: a thing, a group's header, or the row that loads more. */
export type TablePieceItem<T> = { kind: 'item'; key: string; item: T } | TablePieceGroup | { kind: 'loading' }

export type TablePieceProps<T> = {
  label: string
  columns: TableColumn[]
  items: readonly TablePieceItem<T>[]
  cells: (item: T) => Record<string, ReactNode>
  /** What a screen reader calls the row. */
  rowLabel: (item: T) => string
  onOpen: (item: T) => void
  onToggleGroup?: (key: string) => void
  /** Marks the open row. */
  isOpen?: (item: T) => boolean
  hasMore?: boolean
  loadingMore?: boolean
  loadFailed?: boolean
  onLoadMore?: () => void
  /** A last row across the table, such as the one that adds a thing. */
  footer?: ReactNode
  /** Shown under the header when there are no rows. */
  empty?: ReactNode
  /** Above the table: a count, a spinner. */
  summary?: ReactNode
  className?: string
}

/** The table piece. */
export function TablePiece<T>({
  cells,
  className,
  columns,
  empty,
  footer,
  hasMore = false,
  items,
  label,
  loadFailed = false,
  loadingMore = false,
  onLoadMore,
  onOpen,
  onToggleGroup,
  isOpen,
  rowLabel,
  summary,
}: TablePieceProps<T>) {
  const coarse = useCoarsePointer()
  const rowHeight = coarse ? 44 : 36
  const bodyRef = useRef<HTMLDivElement>(null)
  const [active, setActive] = useState<string | null>(null)
  const rows = useMemo<(TablePieceItem<T> | { kind: 'footer' })[]>(
    () => (footer ? [...items, { kind: 'footer' as const }] : [...items]),
    [footer, items],
  )
  const heights = useMemo(() => rows.map(() => rowHeight), [rowHeight, rows])
  const windowed = useWindowedRows(bodyRef, heights)
  const byKey = useMemo(() => {
    const found = new Map<string, T>()
    for (const row of items) if (row.kind === 'item') found.set(row.key, row.item)
    return found
  }, [items])

  // The next page loads when its row scrolls into view.
  const loadingVisible = rows.slice(windowed.start, windowed.end).some((row) => row.kind === 'loading')
  useEffect(() => {
    if (loadingVisible && hasMore && !loadingMore && !loadFailed) onLoadMore?.()
  }, [hasMore, loadFailed, loadingMore, loadingVisible, onLoadMore])

  const toRow = (row: TablePieceItem<T> | { kind: 'footer' }, index: number): TableRow => {
    if (row.kind === 'footer') return { key: 'footer', index, inert: true, height: rowHeight, full: footer }
    if (row.kind === 'loading') {
      return {
        key: 'loading',
        index,
        inert: true,
        height: rowHeight,
        full: loadFailed ? (
          <span className="aw-piece-table__loading">
            More didn’t load.
            <Button onClick={() => onLoadMore?.()} size="sm">
              Try again
            </Button>
          </span>
        ) : (
          <span className="aw-piece-table__loading" role="status">
            <Icon className="aw-spin" name="loader" size={14} /> Loading more…
          </span>
        ),
      }
    }
    if (row.kind === 'group') {
      return {
        key: `group:${row.key}`,
        index,
        height: rowHeight,
        className: 'aw-piece-table__grouprow',
        label: `${row.label || 'None'}, ${countLabel(row.count, 'row')}, ${row.collapsed ? 'collapsed' : 'expanded'}`,
        full: (
          <button aria-expanded={!row.collapsed} className="aw-piece-table__group" onClick={() => onToggleGroup?.(row.key)} tabIndex={-1} type="button">
            <Icon name={row.collapsed ? 'chevron-right' : 'chevron-down'} size={14} />
            {row.color ? <span aria-hidden className="aw-label__dot" style={{ backgroundColor: row.color }} /> : null}
            <span className="aw-piece-table__grouplabel">{row.label || 'None'}</span>
            <span className="aw-piece-table__groupcount">{row.count}</span>
          </button>
        ),
      }
    }
    return {
      key: row.key,
      index,
      height: rowHeight,
      className: isOpen?.(row.item) ? 'is-open' : undefined,
      label: rowLabel(row.item),
      cells: cells(row.item),
    }
  }

  const open = (key: string) => {
    if (key.startsWith('group:')) {
      onToggleGroup?.(key.slice('group:'.length))
      return
    }
    const item = byKey.get(key)
    if (item !== undefined) onOpen(item)
  }

  const shown = rows.slice(windowed.start, windowed.end).map((row, offset) => toRow(row, windowed.start + offset))

  return (
    <div className={['aw-piece-table', className].filter(Boolean).join(' ')} data-piece="table">
      {summary ? (
        <p aria-live="polite" className="aw-piece-table__summary">
          {summary}
        </p>
      ) : null}
      <div className="aw-piece-table__scroll">
        <Table
          // A focused row scrolled out of the window gives the tab stop back to the first one shown.
          activeKey={shown.some((row) => row.key === active) ? active : null}
          bodyRef={bodyRef}
          className="aw-piece-table__table"
          columns={columns}
          label={label}
          onActivate={open}
          onActiveKeyChange={setActive}
          onRowClick={(_event, key) => open(key)}
          rowCount={rows.length}
          rows={shown}
          spaceAfter={windowed.after}
          spaceBefore={windowed.before}
        />
        {items.length === 0 ? empty : null}
      </div>
    </div>
  )
}
