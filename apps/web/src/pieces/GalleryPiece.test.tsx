import { cleanup, fireEvent, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { GalleryPiece } from './GalleryPiece'
import { renderPiece } from './testing'

type Item = { id: string; title: string; owner: string }

const items: Item[] = [
  { id: 'a', title: 'Brief', owner: 'Ana' },
  { id: 'b', title: 'Budget', owner: 'Bo' },
]

afterEach(cleanup)

describe('GalleryPiece', () => {
  it.each([1280, 390])('shows a card for each item, opens one and loads more at %i px', (width) => {
    window.innerWidth = width
    const onOpen = vi.fn()
    const onLoadMore = vi.fn()
    renderPiece(
      <GalleryPiece
        badge={(item) => `#${item.id}`}
        details={(item) => <span>{item.owner}</span>}
        hasMore
        href={(item) => `/items/${item.id}`}
        items={items}
        label="Ideas"
        onLoadMore={onLoadMore}
        onOpen={onOpen}
        openId="b"
        title={(item) => item.title}
      />,
    )
    const cards = within(screen.getByRole('list', { name: 'Ideas' })).getAllByRole('listitem')
    expect(cards.map((card) => card.textContent)).toEqual(['Brief#aAna', 'Budget#bBo'])
    expect(cards[1]).toHaveAttribute('aria-current', 'true')
    fireEvent.click(screen.getByRole('link', { name: 'Brief' }))
    expect(onOpen).toHaveBeenCalledWith(items[0])
    fireEvent.click(screen.getByRole('button', { name: 'Show more' }))
    expect(onLoadMore).toHaveBeenCalledOnce()
  })

  it('shows what it’s given when there are no items', () => {
    renderPiece(<GalleryPiece empty={<p>Nothing yet</p>} href={() => '/'} items={[]} label="Ideas" title={() => ''} />)
    expect(screen.getByText('Nothing yet')).toBeVisible()
    expect(screen.queryByRole('list')).toBeNull()
  })
})
