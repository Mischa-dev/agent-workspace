import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { RevealInThread } from './quotedMessage'
import { ReplyQuote } from './ReplyQuote'

afterEach(cleanup)

const quote = {
  messageId: '019fb8b0-0000-7000-8000-0000000000a1',
  available: true,
  authorName: 'Wren',
  text: 'Can we ship Friday?',
  href: '/messages/dm-1#message-019fb8b0-0000-7000-8000-0000000000a1',
}

function Where() {
  const location = useLocation()
  return <p data-testid="where">{`${location.pathname}${location.search}`}</p>
}

function view(reveal?: (messageId: string) => void) {
  const quoted = <ReplyQuote value={quote} />
  return render(
    <MemoryRouter initialEntries={['/messages/dm-1?thread=thread-1']}>
      <Routes>
        <Route
          element={
            <>
              {reveal ? <RevealInThread.Provider value={reveal}>{quoted}</RevealInThread.Provider> : quoted}
              <Where />
            </>
          }
          path="/messages/:id"
        />
      </Routes>
    </MemoryRouter>,
  )
}

describe('ReplyQuote', () => {
  it('in a thread beside its conversation, looks for an original that is further up the thread there', () => {
    const reveal = vi.fn()
    view(reveal)
    fireEvent.click(screen.getByRole('link', { name: 'Replying to Wren. Go to their message' }))
    expect(reveal).toHaveBeenCalledWith(quote.messageId)
    // The thread stays open.
    expect(screen.getByTestId('where')).toHaveTextContent('/messages/dm-1?thread=thread-1')
  })

  it('elsewhere, follows the original’s link', () => {
    view()
    fireEvent.click(screen.getByRole('link', { name: 'Replying to Wren. Go to their message' }))
    expect(screen.getByTestId('where')).toHaveTextContent('/messages/dm-1')
    expect(screen.getByTestId('where')).not.toHaveTextContent('thread=')
  })
})
