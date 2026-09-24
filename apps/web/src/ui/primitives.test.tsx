import { useState } from 'react'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { Dialog } from './primitives'

function DialogHarness() {
  const [open, setOpen] = useState(false)
  return (
    <>
      <button onClick={() => setOpen(true)} type="button">
        Open dialog
      </button>
      <Dialog label="Example dialog" onClose={() => setOpen(false)} open={open}>
        <button type="button">First action</button>
        <button type="button">Last action</button>
      </Dialog>
    </>
  )
}

describe('Dialog', () => {
  beforeEach(() => {
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
      callback(0)
      return 1
    })
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it('portals above an inert application, contains focus, and restores focus', async () => {
    const applicationRoot = document.createElement('div')
    applicationRoot.id = 'root'
    document.body.appendChild(applicationRoot)
    render(<DialogHarness />, { container: applicationRoot })

    const trigger = screen.getByRole('button', { name: 'Open dialog' })
    trigger.focus()
    fireEvent.click(trigger)

    expect(applicationRoot).toHaveAttribute('inert')
    expect(screen.getByRole('dialog', { name: 'Example dialog' })).not.toBe(
      applicationRoot.querySelector('[role="dialog"]'),
    )
    const first = screen.getByRole('button', { name: 'First action' })
    const last = screen.getByRole('button', { name: 'Last action' })
    expect(first).toHaveFocus()

    fireEvent.keyDown(document, { key: 'Tab', shiftKey: true })
    expect(last).toHaveFocus()
    fireEvent.keyDown(document, { key: 'Tab' })
    expect(first).toHaveFocus()

    fireEvent.keyDown(document, { key: 'Escape' })
    expect(
      screen.queryByRole('dialog', { name: 'Example dialog' }),
    ).not.toBeInTheDocument()
    expect(applicationRoot).not.toHaveAttribute('inert')
    await waitFor(() => expect(trigger).toHaveFocus())
  })
})
