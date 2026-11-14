import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { useRef } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { useGridKeys } from './gridKeys'
import { select, selectAll, NO_SELECTION, keepOnly } from './selection'

function Grid({ onOpen }: { onOpen: (row: number) => void }) {
  const ref = useRef<HTMLTableElement>(null)
  const onKeyDown = useGridKeys(ref, { onOpen, pageRows: 2 })
  const rows = [['A1', 'A2', 'A3'], ['B1', 'B2', 'B3'], ['C1', 'C2', 'C3'], ['D1', 'D2', 'D3']]
  return (
    <table aria-label="Things" onKeyDown={onKeyDown} ref={ref} role="grid">
      <tbody>
        {rows.map((row, rowIndex) => (
          <tr key={row[0]} role="row">
            {row.map((cell, column) => (
              <td key={cell} role="gridcell" tabIndex={rowIndex === 0 && column === 0 ? 0 : -1}>
                {cell === 'B2' ? <input aria-label="B2 value" /> : cell}
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  )
}

describe('grid keys', () => {
  afterEach(cleanup)

  it('moves one tab stop through the cells with the arrows, ends and pages', () => {
    const onOpen = vi.fn()
    render(<Grid onOpen={onOpen} />)
    const cell = (name: string) => screen.getByRole('gridcell', { name })
    cell('A1').focus()
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowRight' })
    expect(document.activeElement).toBe(cell('A2'))
    expect(cell('A2').tabIndex).toBe(0)
    expect(cell('A1').tabIndex).toBe(-1)
    fireEvent.keyDown(document.activeElement!, { key: 'End' })
    expect(document.activeElement).toBe(cell('A3'))
    fireEvent.keyDown(document.activeElement!, { key: 'PageDown' })
    expect(document.activeElement).toBe(cell('C3'))
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' })
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' })
    expect(document.activeElement).toBe(cell('D3'))
    fireEvent.keyDown(document.activeElement!, { key: 'Home', ctrlKey: true })
    expect(document.activeElement).toBe(cell('A1'))
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' })
    fireEvent.keyDown(document.activeElement!, { key: 'Enter' })
    expect(onOpen).toHaveBeenCalledWith(1, expect.any(HTMLElement))
  })

  it('leaves the keys to a field in a cell', () => {
    render(<Grid onOpen={() => undefined} />)
    const field = screen.getByRole('textbox', { name: 'B2 value' })
    field.focus()
    fireEvent.keyDown(field, { key: 'ArrowRight' })
    expect(document.activeElement).toBe(field)
  })
})

describe('selection', () => {
  const order = ['a', 'b', 'c', 'd']

  it('selects one, toggles, and ranges from the anchor', () => {
    const one = select(NO_SELECTION, order, 'b', 'single')
    expect([...one.ids]).toEqual(['b'])
    const range = select(one, order, 'd', 'range')
    expect([...range.ids].sort()).toEqual(['b', 'c', 'd'])
    const toggled = select(range, order, 'c', 'toggle')
    expect([...toggled.ids].sort()).toEqual(['b', 'd'])
  })

  it('drops rows that leave the view, but keeps "all matching"', () => {
    const some = select(select(NO_SELECTION, order, 'a', 'single'), order, 'c', 'range')
    expect([...keepOnly(some, ['b', 'c']).ids].sort()).toEqual(['b', 'c'])
    const all = selectAll(order, true)
    expect(keepOnly(all, ['a'])).toBe(all)
  })
})
