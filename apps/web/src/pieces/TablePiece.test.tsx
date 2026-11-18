import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { TablePiece, type TablePieceItem } from './TablePiece'

type Item = { id: string; title: string; owner: string }

const columns = [
  { id: 'title', header: 'Title', width: '200px' },
  { id: 'owner', header: 'Owner', width: '120px' },
]

const cells = (item: Item) => ({ title: item.title, owner: item.owner })

function rowsOf(items: Item[]): TablePieceItem<Item>[] {
  return items.map((item) => ({ kind: 'item', key: item.id, item }))
}

afterEach(cleanup)

describe('TablePiece', () => {
  it('draws a screenful of 10,000 rows, not all of them', () => {
    const many = Array.from({ length: 10_000 }, (_, index) => ({ id: `t${index}`, title: `Thing ${index}`, owner: 'Ana' }))
    render(<TablePiece cells={cells} columns={columns} items={rowsOf(many)} label="Things" onOpen={() => {}} rowLabel={(item) => item.title} />)
    const grid = screen.getByRole('grid', { name: 'Things' })
    expect(grid).toHaveAttribute('aria-rowcount', '10001')
    expect(grid.querySelectorAll('[role="row"]').length).toBeLessThan(60)
    expect(screen.getByRole('row', { name: 'Thing 0' })).toBeInTheDocument()
    expect(screen.queryByRole('row', { name: 'Thing 9999' })).toBeNull()
  })

  it('opens a row by click or Enter and moves between rows with the arrows', () => {
    const onOpen = vi.fn()
    const items = [
      { id: 'a', title: 'Brief', owner: 'Ana' },
      { id: 'b', title: 'Budget', owner: 'Bo' },
    ]
    render(
      <TablePiece
        cells={cells}
        columns={columns}
        isOpen={(item) => item.id === 'b'}
        items={rowsOf(items)}
        label="Things"
        onOpen={onOpen}
        rowLabel={(item) => item.title}
      />,
    )
    const brief = screen.getByRole('row', { name: 'Brief' })
    const budget = screen.getByRole('row', { name: 'Budget' })
    expect(budget).toHaveClass('is-open')
    expect(brief).toHaveAttribute('tabindex', '0')
    fireEvent.click(within(brief).getByText('Ana'))
    expect(onOpen).toHaveBeenLastCalledWith(items[0])
    fireEvent.keyDown(brief, { key: 'ArrowDown' })
    expect(budget).toHaveAttribute('tabindex', '0')
    fireEvent.keyDown(budget, { key: 'Enter' })
    expect(onOpen).toHaveBeenLastCalledWith(items[1])
  })

  it('folds a group from its header and loads the next page when its row shows', () => {
    const onToggleGroup = vi.fn()
    const onLoadMore = vi.fn()
    const items: TablePieceItem<Item>[] = [
      { kind: 'group', key: 'ana', label: 'Ana', count: 3, collapsed: false },
      { kind: 'item', key: 'a', item: { id: 'a', title: 'Brief', owner: 'Ana' } },
      { kind: 'loading' },
    ]
    render(
      <TablePiece
        cells={cells}
        columns={columns}
        footer={<span>Add a thing</span>}
        hasMore
        items={items}
        label="Things"
        onLoadMore={onLoadMore}
        onOpen={() => {}}
        onToggleGroup={onToggleGroup}
        rowLabel={(item) => item.title}
      />,
    )
    const header = screen.getByRole('row', { name: 'Ana, 3 rows, expanded' })
    fireEvent.keyDown(header, { key: 'Enter' })
    expect(onToggleGroup).toHaveBeenCalledWith('ana')
    expect(onLoadMore).toHaveBeenCalledOnce()
    expect(screen.getByText('Add a thing')).toBeVisible()
  })

  it('shows what it’s given when there are no rows', () => {
    render(<TablePiece cells={cells} columns={columns} empty={<p>Nothing yet</p>} items={[]} label="Things" onOpen={() => {}} rowLabel={(item) => item.title} />)
    expect(screen.getByText('Nothing yet')).toBeVisible()
  })
})
