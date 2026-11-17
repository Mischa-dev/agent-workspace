import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { useMemo, type ReactNode } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ToastViewport } from '../ui'
import { BoardPiece, type BoardColumn, type BoardColumnState, type BoardMoveResult } from './BoardPiece'

type Card = { id: string; title: string; column: string }

const COLUMNS: BoardColumn[] = [
  { key: 'todo', label: 'To do', count: 2 },
  { key: 'doing', label: 'Doing', count: 0 },
  { key: 'done', label: 'Done', count: 3 },
]

type Data = { cards: Card[]; more?: Record<string, () => void> }

/** Each column's cards from a fixed list: what a real source would load. */
function Source({ children, column, source }: { column: BoardColumn; source: Data; children: (state: BoardColumnState<Card>) => ReactNode }) {
  const items = useMemo(() => source.cards.filter((card) => card.column === column.key), [column.key, source.cards])
  const more = source.more?.[column.key]
  return children({
    items,
    pending: false,
    failed: false,
    onRetry: () => undefined,
    hasMore: Boolean(more),
    loadingMore: false,
    onLoadMore: () => more?.(),
    fetchedFrom: 0,
  })
}

const data: Data = {
  cards: [
    { id: 'c1', title: 'Write the brief', column: 'todo' },
    { id: 'c2', title: 'Book the room', column: 'todo' },
  ],
}

function board(onMove?: (card: Card, to: string) => Promise<BoardMoveResult>, source: Data = data) {
  return render(
    <>
      <BoardPiece
        Source={Source}
        columnOf={(card: Card) => card.column}
        columns={COLUMNS}
        href={(card) => `/cards/${card.id}`}
        label="Plan"
        movedTo={(card, key) => ({ ...card, column: key })}
        onMove={onMove}
        source={source}
        title={(card) => card.title}
      />
      <ToastViewport />
    </>,
  )
}

const column = (name: string) => screen.getByRole('region', { name })

/** A pointer event with the fields jsdom's MouseEvent leaves out. */
function pointer(type: string, x: number, y: number) {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, clientX: x, clientY: y })
  Object.defineProperties(event, { pointerId: { value: 1 }, pointerType: { value: 'mouse' } })
  return event
}

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('BoardPiece', () => {
  it('moves a card with its menu, showing it in the new column at once', async () => {
    const onMove = vi.fn(() => new Promise<BoardMoveResult>(() => undefined))
    board(onMove)
    fireEvent.click(within(column('To do')).getByRole('button', { name: 'Move Write the brief' }))
    fireEvent.click(await screen.findByRole('menuitemcheckbox', { name: 'Done' }))
    expect(onMove).toHaveBeenCalledWith(data.cards[0], 'done')
    expect(within(column('Done')).getByRole('link', { name: 'Write the brief' })).toBeVisible()
    expect(within(column('To do')).queryByRole('link', { name: 'Write the brief' })).toBeNull()
  })

  it('moves a card by keyboard: Space picks it up, arrows carry it, Space puts it down', () => {
    const onMove = vi.fn(() => Promise.resolve<BoardMoveResult>(true))
    board(onMove)
    const link = within(column('To do')).getByRole('link', { name: 'Book the room' })
    link.focus()
    fireEvent.keyDown(link, { key: ' ' })
    expect(screen.getByText(/Picked up Book the room/)).toBeInTheDocument()
    fireEvent.keyDown(link, { key: 'ArrowRight' })
    expect(column('Doing')).toHaveClass('is-target')
    fireEvent.keyDown(link, { key: ' ' })
    expect(onMove).toHaveBeenCalledWith(data.cards[1], 'doing')

    // Escape puts a picked-up card back without moving it.
    onMove.mockClear()
    const other = within(column('To do')).getByRole('link', { name: 'Write the brief' })
    fireEvent.keyDown(other, { key: ' ' })
    fireEvent.keyDown(other, { key: 'ArrowRight' })
    fireEvent.keyDown(other, { key: 'Escape' })
    expect(onMove).not.toHaveBeenCalled()
    expect(screen.getByText('Put Write the brief back.')).toBeInTheDocument()
  })

  it('moves a card dragged with a pointer to the column it’s dropped on', () => {
    const onMove = vi.fn(() => Promise.resolve<BoardMoveResult>(true))
    board(onMove)
    const card = within(column('To do')).getByText('Write the brief').closest('li') as HTMLElement
    const done = column('Done')
    Object.defineProperty(document, 'elementFromPoint', { configurable: true, value: () => done })
    act(() => {
      card.dispatchEvent(pointer('pointerdown', 10, 10))
      window.dispatchEvent(pointer('pointermove', 40, 12))
      window.dispatchEvent(pointer('pointermove', 400, 20))
    })
    expect(done).toHaveClass('is-target')
    act(() => {
      window.dispatchEvent(pointer('pointerup', 400, 20))
    })
    expect(onMove).toHaveBeenCalledWith(data.cards[0], 'done')
  })

  it('puts a refused move back and says why', async () => {
    board(() => Promise.resolve<BoardMoveResult>('That column is locked.'))
    fireEvent.click(within(column('To do')).getByRole('button', { name: 'Move Write the brief' }))
    fireEvent.click(await screen.findByRole('menuitemcheckbox', { name: 'Done' }))
    expect(await screen.findByText('That column is locked.')).toBeVisible()
    await waitFor(() => expect(within(column('To do')).getByRole('link', { name: 'Write the brief' })).toBeVisible())
    expect(within(column('Done')).queryByRole('link', { name: 'Write the brief' })).toBeNull()
  })

  it('loads more of one column at a time', () => {
    const more = vi.fn()
    board(undefined, { cards: [{ id: 'd1', title: 'Shipped', column: 'done' }], more: { done: more } })
    expect(within(column('To do')).queryByRole('button', { name: /Show more/ })).toBeNull()
    fireEvent.click(within(column('Done')).getByRole('button', { name: 'Show more (2)' }))
    expect(more).toHaveBeenCalledOnce()
    // Without onMove nothing moves.
    expect(screen.queryByRole('button', { name: /^Move / })).toBeNull()
  })

  it('shows one column at a time when narrow, with a switcher', () => {
    let report: ((entries: { contentRect: { width: number } }[]) => void) | null = null
    vi.stubGlobal(
      'ResizeObserver',
      class {
        constructor(callback: typeof report) {
          report = callback
        }
        observe() {}
        disconnect() {}
      },
    )
    const scrollTo = vi.fn()
    Object.defineProperty(HTMLElement.prototype, 'scrollTo', { configurable: true, value: scrollTo })
    board()
    expect(screen.queryByLabelText('Column')).toBeNull()
    act(() => report?.([{ contentRect: { width: 380 } }]))
    const switcher = screen.getByLabelText('Column')
    expect(within(switcher).getAllByRole('option').map((option) => option.textContent)).toEqual(['To do (2)', 'Doing (0)', 'Done (3)'])
    fireEvent.change(switcher, { target: { value: 'done' } })
    expect(scrollTo).toHaveBeenCalled()
    expect(switcher).toHaveValue('done')
    vi.unstubAllGlobals()
  })
})
