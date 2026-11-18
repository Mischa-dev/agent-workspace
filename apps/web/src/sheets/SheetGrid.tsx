import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ClipboardEvent, type CSSProperties, type KeyboardEvent, type PointerEvent } from 'react'

import type { SheetCell, SheetTab } from './api'
import {
  DEFAULT_WIDTH,
  MAX_COLUMNS,
  MAX_ROWS,
  ROW_HEIGHT,
  bounds,
  cellKey,
  cellLook,
  cellName,
  clamp,
  columnName,
  contains,
  display,
  formatTable,
  parseTable,
  type Position,
  type Selection,
  shownBlock,
} from './sheetModel'
import { markTitle, ruleBook } from './sheetValidation'

/** Rows drawn past the edges of what's in view, so scrolling stays smooth. */
const OVERSCAN = 12

/** A cell's new input; `revision` is the sheet revision the editor opened at. */
export type GridEdit = { row: number; column: number; input: string; revision?: number }

/** What a filter view shows: its rows in order (after the frozen ones) and the columns it hides. */
export type GridShown = { rows: number[]; hiddenColumns: ReadonlySet<number> }

const STYLE_KEYS: Record<string, 'bold' | 'italic' | 'underline'> = { b: 'bold', i: 'italic', u: 'underline' }

/**
 * One tab as a grid: a header of column letters and row numbers that stay
 * put, the frozen rows under it, and only the rows in view drawn. Typing
 * starts an edit, Enter and Tab save it and move on, and copy and paste
 * carry blocks of cells to and from other spreadsheets.
 */
export function SheetGrid({
  cells,
  commented,
  editable,
  onEdits,
  onResize,
  onSelect,
  onStyle,
  pending,
  proposed,
  revision,
  selection,
  shown,
  size,
  tab,
}: {
  cells: Map<string, SheetCell>
  /** Cells a comment thread is about, by `row:column`: they get a corner mark. */
  commented?: ReadonlyMap<string, unknown>
  editable: boolean
  onEdits: (edits: GridEdit[]) => void
  onResize: (column: number, width: number | null) => void
  onSelect: (selection: Selection) => void
  /** Ctrl or Cmd with B, I or U: toggle that style on the selection. */
  onStyle?: (style: 'bold' | 'italic' | 'underline') => void
  /** Inputs sent and not saved yet, shown as typed. */
  pending: Map<string, string>
  /** Cells a waiting proposal would set, outlined. */
  proposed?: ReadonlyMap<string, unknown>
  /** The sheet's revision as shown: an edit holds the one it opened at. */
  revision: number
  selection: Selection
  /** A filter view's rows and hidden columns; all of them when left out. */
  shown?: GridShown | null
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
  // Validation (C1.15): a list gives a cell choices, a checkbox a box, and a cell breaking a lenient rule a mark.
  const rules = useMemo(() => ruleBook(tab.validations ?? []), [tab.validations])
  const width = useCallback((column: number) => (resizing?.column === column ? resizing.width : (tab.columnWidths[column] ?? DEFAULT_WIDTH)), [resizing, tab.columnWidths])
  const hidden = shown?.hiddenColumns
  const columns = useMemo(() => Array.from({ length: size.columns }, (_, index) => index + 1).filter((column) => !hidden?.has(column)), [size.columns, hidden])
  // Body rows by position: in a view, its rows in its order; otherwise every row after the frozen ones.
  const order = shown?.rows ?? null
  const positions = useMemo(() => (order ? new Map(order.map((row, index) => [row, index])) : null), [order])
  const bodyCount = order ? order.length : Math.max(0, size.rows - frozen)
  const rowAt = useCallback((index: number) => (order ? order[index] : frozen + 1 + index), [order, frozen])
  const indexOf = useCallback((row: number) => (positions ? (positions.get(row) ?? -1) : row - frozen - 1), [positions, frozen])

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

  const first = Math.max(0, Math.floor(view.top / ROW_HEIGHT) - OVERSCAN)
  const last = Math.min(bodyCount - 1, first + Math.ceil(view.height / ROW_HEIGHT) + OVERSCAN * 2)
  const body = useMemo(() => Array.from({ length: Math.max(0, last - first + 1) }, (_, index) => rowAt(first + index)), [first, last, rowAt])
  const frozenRows = useMemo(() => Array.from({ length: frozen }, (_, index) => index + 1), [frozen])

  // The focused cell stays in view.
  const focus = selection.focus
  useEffect(() => {
    const element = scroller.current
    if (!element) return
    const headHeight = head.current?.offsetHeight ?? ROW_HEIGHT
    const index = indexOf(focus.row)
    if (focus.row > frozen && index >= 0) {
      const top = index * ROW_HEIGHT
      const visible = element.clientHeight - headHeight
      if (top < element.scrollTop) element.scrollTop = top
      else if (top + ROW_HEIGHT > element.scrollTop + visible) element.scrollTop = top + ROW_HEIGHT - visible
    }
    let left = 0
    for (let column = 1; column < focus.column; column += 1) if (!hidden?.has(column)) left += width(column)
    const right = left + width(focus.column)
    const gutter = 48
    if (left < element.scrollLeft) element.scrollLeft = left
    else if (right + gutter > element.scrollLeft + element.clientWidth) element.scrollLeft = right + gutter - element.clientWidth
  }, [focus, frozen, width, indexOf, hidden])

  // Steps that keep to what's shown: a view's order of rows, past hidden columns.
  const stepRow = (row: number, delta: number) => {
    if (!order) return row + delta
    const index = indexOf(row)
    if (index < 0) return delta > 0 && row >= frozen ? (order[0] ?? row) : row + delta
    if (index + delta < 0) return frozen > 0 ? frozen : row
    return order[Math.min(index + delta, order.length - 1)] ?? row
  }
  const stepColumn = (column: number, delta: number) => {
    let next = column + delta
    while (hidden?.has(next) && next > 1 && next < size.columns) next += delta
    return hidden?.has(next) ? column : next
  }

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

  const inputAt = (row: number, column: number) => pending.get(cellKey(row, column)) ?? cells.get(cellKey(row, column))?.input ?? ''
  /** A checkbox cell flips between TRUE and FALSE. */
  const toggle = (row: number, column: number) => {
    if (!editable) return
    onEdits([{ row, column, input: /^true$/i.test(inputAt(row, column).trim()) ? 'FALSE' : 'TRUE', revision }])
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
      ArrowUp: { row: stepRow(row, -1), column },
      ArrowDown: { row: stepRow(row, 1), column },
      ArrowLeft: { row, column: stepColumn(column, -1) },
      ArrowRight: { row, column: stepColumn(column, 1) },
    }
    if (step[event.key]) {
      event.preventDefault()
      move(step[event.key], extend)
    } else if (event.key === 'Tab') {
      event.preventDefault()
      move({ row, column: stepColumn(column, extend ? -1 : 1) })
    } else if ((event.key === 'Enter' || event.key === ' ') && editable && rules.control(row, column)?.allow === 'checkbox') {
      event.preventDefault()
      toggle(row, column)
    } else if (event.key === 'Enter' || event.key === 'F2') {
      event.preventDefault()
      if (editable) startEdit()
      else move({ row: stepRow(row, 1), column })
    } else if (event.key === 'Delete' || event.key === 'Backspace') {
      event.preventDefault()
      clearSelection()
    } else if (event.key === 'Escape') {
      onSelect({ anchor: focus, focus })
    } else if ((event.metaKey || event.ctrlKey) && !event.altKey && editable && onStyle && STYLE_KEYS[event.key.toLowerCase()]) {
      event.preventDefault()
      onStyle(STYLE_KEYS[event.key.toLowerCase()])
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

  const shownFrom = (row: number, column: number, height: number, breadth: number) => shownBlock({ row, column }, height, breadth, { frozen, order, columns: hidden ? columns : null })

  const onPaste = (event: ClipboardEvent<HTMLDivElement>) => {
    if (editing || !editable) return
    const rows = parseTable(event.clipboardData.getData('text/plain'))
    if (!rows.length) return
    event.preventDefault()
    const box = bounds(selection)
    const top = order && indexOf(selection.focus.row) < indexOf(selection.anchor.row) ? selection.focus.row : order ? selection.anchor.row : box.top
    const breadth = Math.max(...rows.map((line) => line.length))
    const target = shownFrom(top, box.left, rows.length, breadth)
    const edits: GridEdit[] = []
    rows.forEach((line, rowOffset) =>
      line.forEach((input, columnOffset) => {
        const row = target.rows[rowOffset]
        const column = target.columns[columnOffset]
        if (row !== undefined && column !== undefined && row <= MAX_ROWS && column <= MAX_COLUMNS) edits.push({ row, column, input })
      }),
    )
    onEdits(edits)
    const lastRow = target.rows.filter((row): row is number => row !== undefined && row <= MAX_ROWS).at(-1) ?? top
    const lastColumn = target.columns.filter((column): column is number => column !== undefined && column <= MAX_COLUMNS).at(-1) ?? box.left
    onSelect({
      anchor: { row: top, column: box.left },
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
        const control = rules.control(row, column)
        const input = waiting ?? cell?.input ?? ''
        const broken = rules.broken(row, column, input)
        const className = [
          'aw-grid__cell',
          cell?.kind === 'number' && waiting === undefined ? 'is-number' : '',
          cell?.kind === 'error' && waiting === undefined ? 'is-error' : '',
          waiting !== undefined ? 'is-pending' : '',
          selected ? 'is-selected' : '',
          focused ? 'is-focused' : '',
          commented?.has(cellKey(row, column)) ? 'has-comment' : '',
          proposed?.has(key) ? 'is-proposed' : '',
          (cell?.conditional ?? cell?.style)?.fill ? 'has-fill' : '',
          control?.allow === 'list' ? 'has-choices' : '',
          broken ? 'is-invalid' : '',
        ]
          .filter(Boolean)
          .join(' ')
        return (
          <td
            aria-selected={selected}
            className={className}
            key={column}
            onDoubleClick={(event) => {
              // A double-click in the cell's own editor selects a word.
              if (event.target instanceof Element && event.target.closest('.aw-grid__editor')) return
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
            style={cellLook(cell) as CSSProperties | undefined}
            title={broken ? markTitle(broken) : cell?.input.startsWith('=') ? cell.input : (control?.hint ?? undefined)}
          >
            {control?.allow === 'checkbox' && !isEditing ? (
              <input
                aria-label={cellName({ row, column })}
                checked={/^true$/i.test(input.trim())}
                className="aw-grid__check"
                disabled={!editable}
                onChange={() => toggle(row, column)}
                tabIndex={-1}
                type="checkbox"
              />
            ) : isEditing ? (
              <input
                aria-label={`Edit ${cellName({ row, column })}`}
                autoFocus
                className="aw-grid__editor"
                list={control?.allow === 'list' ? `aw-grid-choices-${row}-${column}` : undefined}
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
                    commit({ row: stepRow(row, event.shiftKey ? -1 : 1), column })
                  } else if (event.key === 'Tab') {
                    event.preventDefault()
                    commit({ row, column: stepColumn(column, event.shiftKey ? -1 : 1) })
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
            {isEditing && control?.allow === 'list' ? (
              <datalist id={`aw-grid-choices-${row}-${column}`}>
                {(control.choices ?? []).map((choice) => (
                  <option key={choice} value={choice} />
                ))}
              </datalist>
            ) : null}
          </td>
        )
      })}
    </tr>
  )

  const above = first * ROW_HEIGHT
  const below = Math.max(0, bodyCount - 1 - last) * ROW_HEIGHT

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
