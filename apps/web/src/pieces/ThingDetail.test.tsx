import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ThingDetail } from './ThingDetail'

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

/** A phone-wide screen, as `usePhoneWidth` asks for it. */
function onPhone() {
  vi.stubGlobal('matchMedia', (query: string) => ({
    matches: true,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  }))
}

describe('ThingDetail', () => {
  it('is a panel that closes with its button or Escape, but not from inside a field', () => {
    const onClose = vi.fn()
    render(
      <ThingDetail eyebrow="Ideas #4" label="Brief" onClose={onClose}>
        <input aria-label="Title" />
      </ThingDetail>,
    )
    const panel = screen.getByRole('complementary', { name: 'Brief' })
    expect(panel).toHaveTextContent('Ideas #4')
    fireEvent.keyDown(screen.getByRole('textbox', { name: 'Title' }), { key: 'Escape' })
    expect(onClose).not.toHaveBeenCalled()
    fireEvent.keyDown(panel, { key: 'Escape' })
    expect(onClose).toHaveBeenCalledOnce()
    fireEvent.click(screen.getByRole('button', { name: 'Close' }))
    expect(onClose).toHaveBeenCalledTimes(2)
  })

  it('is a whole-screen sheet with Back on a phone', () => {
    onPhone()
    const onClose = vi.fn()
    render(<ThingDetail label="Brief" onClose={onClose} />)
    expect(screen.queryByRole('button', { name: 'Close' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Back' }))
    expect(onClose).toHaveBeenCalledOnce()
  })

  it('is a page with no close, and its tabs move with the arrow keys', () => {
    render(
      <ThingDetail
        as="page"
        label="Brief"
        tabs={[
          { id: 'fields', label: 'Fields', content: <p>Owner: Ana</p> },
          { id: 'history', label: 'History', content: <p>Ana changed it</p> },
        ]}
      />,
    )
    expect(screen.getByRole('article', { name: 'Brief' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Close' })).toBeNull()
    expect(screen.getByRole('tabpanel')).toHaveTextContent('Owner: Ana')
    fireEvent.keyDown(screen.getByRole('tab', { name: 'Fields' }), { key: 'ArrowRight' })
    expect(screen.getByRole('tab', { name: 'History' })).toHaveAttribute('aria-selected', 'true')
    expect(screen.getByRole('tabpanel')).toHaveTextContent('Ana changed it')
  })
})
