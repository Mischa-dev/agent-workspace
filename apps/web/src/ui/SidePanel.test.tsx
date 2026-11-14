import { act, cleanup, fireEvent, screen } from '@testing-library/react'
import { useState } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { axeViolations, renderPiece } from '../pieces/testing'
import { SidePanel } from './SidePanel'

function Harness({ onClose = () => undefined }: { onClose?: () => void }) {
  const [open, setOpen] = useState(false)
  return (
    <div>
      <button onClick={() => setOpen(true)} type="button">
        Open details
      </button>
      {open ? (
        <SidePanel
          onClose={() => {
            onClose()
            setOpen(false)
          }}
          title="Details"
        >
          <label>
            Note <input />
          </label>
          <button type="button">Inside</button>
        </SidePanel>
      ) : null}
    </div>
  )
}

describe('SidePanel', () => {
  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it('closes on Escape and gives focus back to what opened it', async () => {
    const { container } = renderPiece(<Harness />)
    const opener = screen.getByRole('button', { name: 'Open details' })
    opener.focus()
    fireEvent.click(opener)
    const inside = screen.getByRole('button', { name: 'Inside' })
    act(() => inside.focus())
    expect(await axeViolations(container)).toEqual([])
    fireEvent.keyDown(inside, { key: 'Escape' })
    expect(screen.queryByRole('complementary')).toBeNull()
    expect(document.activeElement).toBe(opener)
  })

  it('leaves Escape to a field that has the key', () => {
    const onClose = vi.fn()
    renderPiece(<Harness onClose={onClose} />)
    fireEvent.click(screen.getByRole('button', { name: 'Open details' }))
    fireEvent.keyDown(screen.getByRole('textbox', { name: 'Note' }), { key: 'Escape' })
    expect(onClose).not.toHaveBeenCalled()
    expect(screen.getByRole('complementary', { name: 'Details' })).toBeInTheDocument()
  })

  it('opens as a sheet on a phone, and closes from its button', () => {
    const onClose = vi.fn()
    renderPiece(<Harness onClose={onClose} />, { width: 'narrow' })
    fireEvent.click(screen.getByRole('button', { name: 'Open details' }))
    const sheet = screen.getByRole('dialog', { name: 'Details' })
    expect(sheet).toHaveAttribute('aria-modal', 'true')
    expect(sheet.contains(document.activeElement)).toBe(true)
    fireEvent.click(screen.getByRole('button', { name: 'Close' }))
    expect(onClose).toHaveBeenCalledOnce()
  })
})
