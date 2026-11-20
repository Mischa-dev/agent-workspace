import { cleanup, fireEvent, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ListPiece, ListPieceTitle } from './ListPiece'
import { axeViolations, renderPiece, type Width } from './testing'

type Item = { id: string; title: string; where: string; done?: boolean; href?: string }

const items: Item[] = [
  { id: 'a', title: 'Write the brief', where: 'Launch', href: '/items/a' },
  { id: 'b', title: 'Call the printer', where: 'Reminder' },
  { id: 'c', title: 'Book the room', where: 'Launch', href: '/items/c', done: true },
]

afterEach(cleanup)

describe('ListPiece', () => {
  it.each<Width>(['wide', 'narrow'])('draws a row for each item with its parts at %s width', async (width) => {
    const onDone = vi.fn()
    const { container } = renderPiece(
      <ListPiece
        actions={(item) => (
          <button onClick={() => onDone(item.id)} type="button">
            Done: {item.title}
          </button>
        )}
        aside={(item) => <span>{item.where}</span>}
        isDone={(item) => Boolean(item.done)}
        isOpen={(item) => item.id === 'a'}
        itemKey={(item) => item.id}
        items={items}
        label="Saved"
        leading={() => <span aria-hidden="true">•</span>}
        meta={(item) => `From ${item.where}`}
        title={(item) => <ListPieceTitle to={item.href}>{item.title}</ListPieceTitle>}
      />,
      { width },
    )
    const rows = within(screen.getByRole('list', { name: 'Saved' })).getAllByRole('listitem')
    expect(rows).toHaveLength(3)
    expect(rows[0]).toHaveClass('is-open')
    expect(rows[2]).toHaveClass('is-done')
    expect(within(rows[0]).getByRole('link', { name: 'Write the brief' })).toHaveAttribute('href', '/items/a')
    // An item with nowhere to open has a plain title.
    expect(within(rows[1]).queryByRole('link')).toBeNull()
    expect(rows[1]).toHaveTextContent('From Reminder')
    fireEvent.click(within(rows[1]).getByRole('button', { name: 'Done: Call the printer' }))
    expect(onDone).toHaveBeenCalledWith('b')
    expect(await axeViolations(container)).toEqual([])
  })

  it('shows what it is given when there are no items, and can be named by a heading', () => {
    renderPiece(
      <>
        <h2 id="mine">Mine</h2>
        <ListPiece
          empty={<p>Nothing here</p>}
          itemKey={(item: Item) => item.id}
          items={[]}
          labelledBy="mine"
          title={(item) => item.title}
        />
        <ListPiece itemKey={(item: Item) => item.id} items={items.slice(0, 1)} labelledBy="mine" title={(item) => item.title} />
      </>,
    )
    expect(screen.getByText('Nothing here')).toBeInTheDocument()
    expect(screen.getByRole('list', { name: 'Mine' })).toBeInTheDocument()
  })
})
