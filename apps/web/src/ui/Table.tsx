import { useRef, type CSSProperties, type KeyboardEvent, type MouseEvent, type ReactNode, type RefObject } from 'react'

import './pickers.css'

export type TableColumn = {
  id: string
  header: ReactNode
  /** A CSS grid track, such as `minmax(0, 1fr)` or `96px`. */
  width?: string
  align?: 'start' | 'end'
  /** Hidden below 640px wide so phones keep the essentials. */
  hideOnPhone?: boolean
  /** Hides the header text (the column still has a name for screen readers). */
  quietHeader?: boolean
}

export type TableRow = {
  key: string
  /** 1-based position among all rows, for windowed tables. */
  index?: number
  cells?: Record<string, ReactNode>
  /** A row with one cell across the table, such as a group header. */
  full?: ReactNode
  selected?: boolean
  /** Rows that can't take focus, such as a loading row. */
  inert?: boolean
  className?: string
  label?: string
  height?: number
}

const track = (column: TableColumn) => column.width ?? 'minmax(0, 1fr)'

const columnsStyle = (columns: TableColumn[]) =>
  ({
    '--aw-table-columns': columns.map(track).join(' '),
    '--aw-table-columns-phone': columns.filter((column) => !column.hideOnPhone).map(track).join(' '),
  }) as CSSProperties

/**
 * A data grid with a sticky header and keyboard row movement: Up and Down (or
 * J and K when `letterKeys` is on), Home, End, Page Up and Page Down move
 * between rows and Enter activates one. Rows can be windowed: pass only the
 * rows in view, with `rowCount`, `index` and the space above and below.
 */
export function Table({
  activeKey,
  ariaMultiselectable,
  className,
  columns,
  empty,
  bodyRef,
  label,
  onActivate,
  onActiveKeyChange,
  onRowClick,
  onRowKeyDown,
  rowCount,
  rows,
  spaceAfter = 0,
  spaceBefore = 0,
}: {
  activeKey?: string | null
  ariaMultiselectable?: boolean
  className?: string
  columns: TableColumn[]
  empty?: ReactNode
  /** The element that holds the rows, for windowing. */
  bodyRef?: RefObject<HTMLDivElement | null>
  label: string
  onActivate?: (key: string) => void
  /** Focus moved to another row. Windowed tables scroll it into view. */
  onActiveKeyChange?: (key: string) => void
  /** A click on the row itself, not on a control inside it. */
  onRowClick?: (event: MouseEvent<HTMLDivElement>, key: string) => void
  /** Keys the table doesn't handle itself, for shortcuts on the focused row. */
  onRowKeyDown?: (event: KeyboardEvent<HTMLDivElement>, key: string) => void
  rowCount?: number
  rows: TableRow[]
  spaceAfter?: number
  spaceBefore?: number
}) {
  const ownBody = useRef<HTMLDivElement>(null)
  const body = bodyRef ?? ownBody
  const focusable = rows.filter((row) => !row.inert)
  const current = activeKey ?? focusable[0]?.key ?? null

  const move = (event: KeyboardEvent<HTMLDivElement>, key: string) => {
    const position = focusable.findIndex((row) => row.key === key)
    const steps: Record<string, number> = { ArrowDown: 1, ArrowUp: -1, PageDown: 10, PageUp: -10 }
    let next: number | null = null
    if (event.key in steps) next = position + steps[event.key]
    else if (event.key === 'Home') next = 0
    else if (event.key === 'End') next = focusable.length - 1
    if (next === null) return false
    event.preventDefault()
    const target = focusable[Math.max(0, Math.min(next, focusable.length - 1))]
    if (!target) return true
    onActiveKeyChange?.(target.key)
    window.requestAnimationFrame(() => body.current?.querySelector<HTMLElement>(`[data-row-key="${CSS.escape(target.key)}"]`)?.focus())
    return true
  }

  return (
    <div
      aria-label={label}
      aria-multiselectable={ariaMultiselectable || undefined}
      aria-rowcount={rowCount !== undefined ? rowCount + 1 : undefined}
      className={['aw-table', className].filter(Boolean).join(' ')}
      role="grid"
      style={columnsStyle(columns)}
    >
      <div className="aw-table__head" role="rowgroup">
        <div aria-rowindex={rowCount !== undefined ? 1 : undefined} className="aw-table__row aw-table__row--head" role="row">
          {columns.map((column) => (
            <div
              className={['aw-table__cell', column.align === 'end' ? 'is-end' : '', column.hideOnPhone ? 'is-wide-only' : ''].filter(Boolean).join(' ')}
              key={column.id}
              role="columnheader"
            >
              {column.quietHeader ? <span className="visually-hidden">{column.header}</span> : column.header}
            </div>
          ))}
        </div>
      </div>
      <div className="aw-table__body" ref={body} role="rowgroup">
        {spaceBefore ? <div aria-hidden="true" style={{ height: spaceBefore }} /> : null}
        {rows.map((row) => (
          <div
            aria-label={row.label}
            aria-rowindex={row.index !== undefined ? row.index + 1 : undefined}
            aria-selected={row.selected === undefined ? undefined : row.selected}
            className={['aw-table__row', row.full ? 'aw-table__row--full' : '', row.selected ? 'is-selected' : '', row.className].filter(Boolean).join(' ')}
            data-row-key={row.key}
            key={row.key}
            onClick={(event) => {
              if (!onRowClick || row.inert) return
              if ((event.target as HTMLElement).closest('a, button, input, select, textarea, label')) return
              onRowClick(event, row.key)
            }}
            onFocus={(event) => {
              if (event.target === event.currentTarget && row.key !== activeKey && !row.inert) onActiveKeyChange?.(row.key)
            }}
            onKeyDown={(event) => {
              if (event.target !== event.currentTarget || row.inert) return
              if (move(event, row.key)) return
              if (event.key === 'Enter' && onActivate) {
                event.preventDefault()
                onActivate(row.key)
                return
              }
              onRowKeyDown?.(event, row.key)
            }}
            role="row"
            style={row.height ? { height: row.height } : undefined}
            tabIndex={row.inert ? undefined : row.key === current ? 0 : -1}
          >
            {row.full ? (
              <div className="aw-table__cell aw-table__cell--full" role="gridcell">
                {row.full}
              </div>
            ) : (
              columns.map((column) => (
                <div
                  className={['aw-table__cell', column.align === 'end' ? 'is-end' : '', column.hideOnPhone ? 'is-wide-only' : ''].filter(Boolean).join(' ')}
                  key={column.id}
                  role="gridcell"
                >
                  {row.cells?.[column.id]}
                </div>
              ))
            )}
          </div>
        ))}
        {spaceAfter ? <div aria-hidden="true" style={{ height: spaceAfter }} /> : null}
        {rows.length === 0 && empty ? <div className="aw-table__empty">{empty}</div> : null}
      </div>
    </div>
  )
}
