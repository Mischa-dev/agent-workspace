import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ClipboardEvent, type KeyboardEvent, type PointerEvent } from 'react'

import type { SheetCell, SheetTab } from './api'
import {
  DEFAULT_WIDTH,
  MAX_COLUMNS,
  MAX_ROWS,
  ROW_HEIGHT,
  bounds,
  cellKey,
  cellName,
  clamp,
  columnName,
  contains,
  display,
  formatTable,
  parseTable,
  type Position,
  type Selection,
} from './sheetModel'

/** Rows drawn past the edges of what's in view, so scrolling stays smooth. */
const OVERSCAN = 12

/** A cell's new input; `revision` is the sheet revision the editor opened at. */
export type GridEdit = { row: number; column: number; input: string; revision?: number }

/**
 * One tab as a grid: a header of column letters and row numbers that stay
 * put, the frozen rows under it, and only the rows in view drawn. Typing
 * starts an edit, Enter and Tab save it and move on, and copy and paste
 * carry blocks of cells to and from other spreadsheets.
 */
export function SheetGrid({
  cells,
  editable,
  onEdits,
  onResize,
  onSelect,
  pending,
  revision,
  selection,
  size,
  tab,
}: {
  cells: Map<string, SheetCell>
  editable: boolean
  onEdits: (edits: GridEdit[]) => void
  onResize: (column: number, width: number | null) => void
  onSelect: (selection: Selection) => void
  /** Inputs sent and not saved yet, shown as typed. */
  pending: Map<string, string>
  /** The sheet's revision as shown: an edit holds the one it opened at. */
  revision: number
  selection: Selection
  size: { rows: number; columns: number }
  tab: SheetTab
}) {
  const scroller = useRef<HTMLDivElement>(null)
  const head = useRef<HTMLTableSectionElement>(null)
  const [view, setView] = useState({ top: 0, height: 600 })
  const [editing, setEditing] = useState<{
    position: Position
    text: string
    revision: number
  } | null>(null)
  const committed = useRef(false)
  const [dragging, setDragging] = useState(false)
  const [resizing, setResizing] = useState<{
    column: number
    width: number
  } | null>(null)
  const frozen = Math.min(tab.frozenRows, size.rows)
  const width = useCallback((column: number) => (resizing?.column === column ? resizing.width : (tab.columnWidths[column] ?? DEFAULT_WIDTH)), [resizing, tab.columnWidths])
  const columns = useMemo(() => Array.from({ length: size.columns }, (_, index) => index + 1), [size.columns])

  // What's in view, kept as the grid scrolls and resizes.
  useLayoutEffect(() => {
    const element = scroller.current
    if (!element) return undefined
    let frame = 0
    const measure = () => {
      frame = 0
      setView({ top: element.scrollTop, height: element.clientHeight || 600 })
    }
    const onScroll = () => {
      if (!frame) frame = window.requestAnimationFrame(measure)
    }
    measure()
    element.addEventListener('scroll', onScroll, { passive: true })
    const observer = typeof ResizeObserver === 'function' ? new ResizeObserver(onScroll) : null
    observer?.observe(element)
    return () => {
      element.removeEventListener('scroll', onScroll)
      observer?.disconnect()
      if (frame) window.cancelAnimationFrame(frame)
    }
  }, [])

  const first = Math.max(frozen + 1, frozen + 1 + Math.floor(view.top / ROW_HEIGHT) - OVERSCAN)
  const last = Math.min(size.rows, first + Math.ceil(view.height / ROW_HEIGHT) + OVERSCAN * 2)
  const body = useMemo(() => Array.from({ length: Math.max(0, last - first + 1) }, (_, index) => first + index), [first, last])
  const frozenRows = useMemo(() => Array.from({ length: frozen }, (_, index) => index + 1), [frozen])

  // The focused cell stays in view.
  const focus = selection.focus
  useEffect(() => {
    const element = scroller.current
    if (!element) return
    const headHeight = head.current?.offsetHeight ?? ROW_HEIGHT
    if (focus.row > frozen) {
      const top = (focus.row - frozen - 1) * ROW_HEIGHT
      const visible = element.clientHeight - headHeight
      if (top < element.scrollTop) element.scrollTop = top
      else if (top + ROW_HEIGHT > element.scrollTop + visible) element.scrollTop = top + ROW_HEIGHT - visible
    }
    let left = 0
    for (let column = 1; column < focus.column; column += 1) left += width(column)
    const right = left + width(focus.column)
    const gutter = 48
    if (left < element.scrollLeft) element.scrollLeft = left
    else if (right + gutter > element.scrollLeft + element.clientWidth) element.scrollLeft = right + gutter - element.clientWidth
  }, [focus, frozen, width])

  const move = (to: Position, extend = false) => {
    const next = clamp(to, {
      rows: MAX_ROWS,
      columns: Math.min(MAX_COLUMNS, size.columns),
    })
    onSelect(extend ? { anchor: selection.anchor, focus: next } : { anchor: next, focus: next })
  }

  const startEdit = (text?: string) => {
    if (!editable) return
    committed.current = false
    const current = cells.get(cellKey(focus.row, focus.column))
    setEditing({
      position: focus,
      text: text ?? pending.get(cellKey(focus.row, focus.column)) ?? current?.input ?? '',
      revision,
    })
  }

  const commit = (then?: Position) => {
    // Enter and Tab save, and the editor's blur that follows must not save again.
    if (!editing || committed.current) return
    committed.current = true
    const { position, text } = editing
    const before = pending.get(cellKey(position.row, position.column)) ?? cells.get(cellKey(position.row, position.column))?.input ?? ''
    setEditing(null)
    if (text !== before) onEdits([{ row: position.row, column: position.column, input: text, revision: editing.revision }])
    if (then) move(then)
    scroller.current?.focus()
  }

  const clearSelection = () => {
    if (!editable) return
    const box = bounds(selection)
    const edits: GridEdit[] = []
    for (let row = box.top; row <= box.bottom; row += 1) {
      for (let column = box.left; column <= box.right; column += 1) {
        if (cells.has(cellKey(row, column)) || pending.get(cellKey(row, column))) edits.push({ row, column, input: '' })
      }
    }
    if (edits.length) onEdits(edits)
  }

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (editing) return
    const { row, column } = focus
    const extend = event.shiftKey
    const step: Record<string, Position> = {
      ArrowUp: { row: row - 1, column },
      ArrowDown: { row: row + 1, column },
      ArrowLeft: { row, column: column - 1 },
      ArrowRight: { row, column: column + 1 },
    }
    if (step[event.key]) {
      event.preventDefault()
      move(step[event.key], extend)
    } else if (event.key === 'Tab') {
      event.preventDefault()
      move({ row, column: column + (extend ? -1 : 1) })
    } else if (event.key === 'Enter' || event.key === 'F2') {
      event.preventDefault()
      if (editable) startEdit()
      else move({ row: row + 1, column })
    } else if (event.key === 'Delete' || event.key === 'Backspace') {
      event.preventDefault()
      clearSelection()
    } else if (event.key === 'Escape') {
      onSelect({ anchor: focus, focus })
    } else if (event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey && editable) {
      event.preventDefault()
      startEdit(event.key)
    }
  }

  const onCopy = (event: ClipboardEvent<HTMLDivElement>, cut = false) => {
    if (editing) return
    const box = bounds(selection)
    const rows: string[][] = []
    for (let row = box.top; row <= box.bottom; row += 1) {
      const line: string[] = []
      for (let column = box.left; column <= box.right; column += 1) line.push(pending.get(cellKey(row, column)) ?? cells.get(cellKey(row, column))?.input ?? '')
      rows.push(line)
    }
    event.clipboardData.setData('text/plain', formatTable(rows))
    event.preventDefault()
    if (cut) clearSelection()
  }

  const onPaste = (event: ClipboardEvent<HTMLDivElement>) => {
    if (editing || !editable) return
    const rows = parseTable(event.clipboardData.getData('text/plain'))
    if (!rows.length) return
    event.preventDefault()
    const box = bounds(selection)
    const edits: GridEdit[] = []
    rows.forEach((line, rowOffset) =>
      line.forEach((input, columnOffset) => {
        const row = box.top + rowOffset
        const column = box.left + columnOffset
        if (row <= MAX_ROWS && column <= MAX_COLUMNS) edits.push({ row, column, input })
      }),
    )
    onEdits(edits)
    const lastRow = Math.min(MAX_ROWS, box.top + rows.length - 1)
    const lastColumn = Math.min(MAX_COLUMNS, box.left + Math.max(...rows.map((line) => line.length)) - 1)
    onSelect({
      anchor: { row: box.top, column: box.left },
      focus: { row: lastRow, column: lastColumn },
    })
  }

  const onPointerDown = (event: PointerEvent<HTMLTableCellElement>, position: Position) => {
    if (event.button !== 0) return
    // A click in the cell's own editor moves the caret or selects text.
    if (event.target instanceof Element && event.target.closest('.aw-grid__editor')) return
    if (editing) commit()
    move(position, event.shiftKey)
    setDragging(true)
  }

  const startResize = (event: PointerEvent<HTMLSpanElement>, column: number) => {
    event.preventDefault()
    event.stopPropagation()
    const startX = event.clientX
    const startWidth = width(column)
    const target = event.currentTarget
    target.setPointerCapture(event.pointerId)
    const onMove = (moveEvent: globalThis.PointerEvent) =>
      setResizing({
        column,
        width: Math.min(800, Math.max(40, Math.round(startWidth + moveEvent.clientX - startX))),
      })
    const onUp = (upEvent: globalThis.PointerEvent) => {
      target.removeEventListener('pointermove', onMove)
      target.removeEventListener('pointerup', onUp)
      const next = Math.min(800, Math.max(40, Math.round(startWidth + upEvent.clientX - startX)))
      setResizing(null)
      if (next !== startWidth) onResize(column, next === DEFAULT_WIDTH ? null : next)
    }
    target.addEventListener('pointermove', onMove)
    target.addEventListener('pointerup', onUp)
  }

  const renderRow = (row: number) => (
    <tr key={row} style={{ height: ROW_HEIGHT }}>
      <th className="aw-grid__rowhead" scope="row">
        {row}
      </th>
      {columns.map((column) => {
        const key = cellKey(row, column)
        const cell = cells.get(key)
        const waiting = pending.get(key)
        const selected = contains(selection, row, column)
        const focused = focus.row === row && focus.column === column
        const isEditing = editing && editing.position.row === row && editing.position.column === column
        const className = [
          'aw-grid__cell',
          cell?.kind === 'number' && waiting === undefined ? 'is-number' : '',
          cell?.kind === 'error' && waiting === undefined ? 'is-error' : '',
          waiting !== undefined ? 'is-pending' : '',
          selected ? 'is-selected' : '',
          focused ? 'is-focused' : '',
        ]
          .filter(Boolean)
          .join(' ')
        return (
          <td
            aria-selected={selected}
            className={className}
            key={column}
            onDoubleClick={() => {
              move({ row, column })
              committed.current = false
              if (editable)
                setEditing({
                  position: { row, column },
                  text: waiting ?? cell?.input ?? '',
                  revision,
                })
            }}
            onPointerDown={(event) => onPointerDown(event, { row, column })}
            onPointerEnter={() => {
              if (dragging) onSelect({ anchor: selection.anchor, focus: { row, column } })
            }}
            title={cell?.input.startsWith('=') ? cell.input : undefined}
          >
            {isEditing ? (
              <input
                aria-label={`Edit ${cellName({ row, column })}`}
                autoFocus
                className="aw-grid__editor"
                onBlur={() => commit()}
                onChange={(event) =>
                  setEditing({
                    ...editing,
                    text: event.target.value,
                  })
                }
                onKeyDown={(event) => {
                  if (event.key === 'Enter') {
                    event.preventDefault()
                    commit({ row: row + (event.shiftKey ? -1 : 1), column })
                  } else if (event.key === 'Tab') {
                    event.preventDefault()
                    commit({ row, column: column + (event.shiftKey ? -1 : 1) })
                  } else if (event.key === 'Escape') {
                    event.preventDefault()
                    committed.current = true
                    setEditing(null)
                    scroller.current?.focus()
                  }
                }}
                value={editing.text}
              />
            ) : (
              <span className="aw-grid__text">{waiting ?? display(cell)}</span>
            )}
          </td>
        )
      })}
    </tr>
  )

  const above = (first - frozen - 1) * ROW_HEIGHT
  const below = (size.rows - last) * ROW_HEIGHT

  return (
    <div
      aria-label={`${tab.name}, cell ${cellName(focus)}`}
      aria-multiselectable
      className={['aw-grid', dragging ? 'is-dragging' : ''].filter(Boolean).join(' ')}
      onCopy={(event) => onCopy(event)}
      onCut={(event) => onCopy(event, true)}
      onKeyDown={onKeyDown}
      onPaste={onPaste}
      onPointerLeave={() => setDragging(false)}
      onPointerUp={() => setDragging(false)}
      ref={scroller}
      role="grid"
      tabIndex={0}
    >
      <table className="aw-grid__table">
        <colgroup>
          <col style={{ width: 48 }} />
          {columns.map((column) => (
            <col key={column} style={{ width: width(column) }} />
          ))}
        </colgroup>
        <thead className="aw-grid__head" ref={head}>
          <tr style={{ height: ROW_HEIGHT }}>
            <th aria-label="Rows and columns" className="aw-grid__corner" />
            {columns.map((column) => (
              <th
                className={[
                  'aw-grid__colhead',
                  column >= Math.min(selection.anchor.column, focus.column) && column <= Math.max(selection.anchor.column, focus.column) ? 'is-active' : '',
                ].join(' ')}
                key={column}
                scope="col"
              >
                {columnName(column)}
                {editable ? <span aria-hidden className="aw-grid__resize" onPointerDown={(event) => startResize(event, column)} /> : null}
              </th>
            ))}
          </tr>
          {frozenRows.map(renderRow)}
        </thead>
        <tbody>
          {above > 0 ? (
            <tr aria-hidden style={{ height: above }}>
              <td colSpan={columns.length + 1} />
            </tr>
          ) : null}
          {body.map(renderRow)}
          {below > 0 ? (
            <tr aria-hidden style={{ height: below }}>
              <td colSpan={columns.length + 1} />
            </tr>
          ) : null}
        </tbody>
      </table>
    </div>
  )
}
