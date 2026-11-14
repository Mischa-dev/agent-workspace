import { useCallback, type KeyboardEvent, type RefObject } from 'react'

import { inOwnControl } from './focusRule'

/**
 * Keys for a grid of dense data (the WAI-ARIA grid pattern, with a roving
 * tab index): one cell is in the tab order; the arrows move between cells,
 * Home and End to the ends of a row, Ctrl+Home and Ctrl+End to the first and
 * last cell, Page Up and Page Down by `pageRows`. Enter opens the row.
 * Cells are `[role="gridcell"]` or `[role="columnheader"]` inside
 * `[role="row"]`. A field or menu in a cell keeps its own keys.
 */
export function useGridKeys(
  gridRef: RefObject<HTMLElement | null>,
  { onOpen, pageRows = 10 }: { onOpen?: (rowIndex: number, row: HTMLElement) => void; pageRows?: number } = {},
) {
  return useCallback(
    (event: KeyboardEvent<HTMLElement>) => {
      const grid = gridRef.current
      if (!grid || inOwnControl(event.target)) return
      const rows = [...grid.querySelectorAll<HTMLElement>('[role="row"]')]
      const cellsOf = (row: HTMLElement) => [...row.querySelectorAll<HTMLElement>('[role="gridcell"], [role="columnheader"]')]
      const active = document.activeElement
      const rowIndex = rows.findIndex((row) => active instanceof Node && row.contains(active))
      if (rowIndex < 0) return
      const cells = cellsOf(rows[rowIndex])
      const column = Math.max(0, cells.findIndex((cell) => cell === active || (active instanceof Node && cell.contains(active))))
      let nextRow = rowIndex
      let nextColumn = column
      switch (event.key) {
        case 'ArrowDown':
          nextRow = Math.min(rows.length - 1, rowIndex + 1)
          break
        case 'ArrowUp':
          nextRow = Math.max(0, rowIndex - 1)
          break
        case 'ArrowRight':
          nextColumn = Math.min(cells.length - 1, column + 1)
          break
        case 'ArrowLeft':
          nextColumn = Math.max(0, column - 1)
          break
        case 'PageDown':
          nextRow = Math.min(rows.length - 1, rowIndex + pageRows)
          break
        case 'PageUp':
          nextRow = Math.max(0, rowIndex - pageRows)
          break
        case 'Home':
          nextColumn = 0
          if (event.ctrlKey) nextRow = 0
          break
        case 'End':
          nextColumn = Number.MAX_SAFE_INTEGER
          if (event.ctrlKey) nextRow = rows.length - 1
          break
        case 'Enter':
          if (onOpen) {
            event.preventDefault()
            onOpen(rowIndex, rows[rowIndex])
          }
          return
        default:
          return
      }
      event.preventDefault()
      const target = cellsOf(rows[nextRow])
      const cell = target[Math.min(nextColumn, target.length - 1)]
      if (!cell) return
      for (const each of grid.querySelectorAll<HTMLElement>('[role="gridcell"], [role="columnheader"]')) {
        each.tabIndex = each === cell ? 0 : -1
      }
      cell.focus()
    },
    [gridRef, onOpen, pageRows],
  )
}
