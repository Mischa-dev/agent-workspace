import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import FrameHost, { type FrameAnswer } from './FrameHost'

const callOp = vi.fn()
vi.mock('../../api/ops', async (original) => ({
  ...(await original<typeof import('../../api/ops')>()),
  callOp: (name: string, input: unknown, options: unknown) => callOp(name, input, options),
}))

const origin = 'https://keys.example.com'
const frame: FrameAnswer = {
  kind: 'frame',
  launchId: '00000000-0000-4000-8000-000000000001',
  address: '/panel-frame/00000000-0000-4000-8000-000000000001',
  origin,
  title: 'Model keys',
  height: 480,
}

afterEach(() => {
  cleanup()
  callOp.mockReset()
})

// The host page isn't loaded in tests, so put the app's frame inside it by hand.
function appWindow(): Window {
  const outer = screen.getByTitle('Model keys') as HTMLIFrameElement
  const document = outer.contentDocument as Document
  document.write('<!doctype html><body><iframe></iframe></body>')
  document.close()
  return document.querySelector('iframe')?.contentWindow as Window
}

function send(data: unknown, from: { origin: string; source: Window | null }) {
  act(() => {
    window.dispatchEvent(
      new MessageEvent('message', {
        data,
        origin: from.origin,
        source: from.source,
      }),
    )
  })
}

describe('FrameHost', () => {
  it('hears only the app’s own frame at the app’s origin', async () => {
    const onAnswer = vi.fn()
    const onCancel = vi.fn()
    render(<FrameHost frame={frame} onAnswer={onAnswer} onCancel={onCancel} />)
    const app = appWindow()
    const frameElement = screen.getByTitle('Model keys')
    expect(frameElement).toHaveAttribute('src', `${frame.address}?scheme=light`)
    expect(frameElement).toHaveStyle({ height: '480px' })

    // Another origin, or the right origin from another window, is ignored.
    send({ aw: 1, type: 'done', ref: 'saved-1' }, { origin: 'https://elsewhere.example', source: app })
    send({ aw: 1, type: 'done', ref: 'saved-1' }, { origin, source: window })
    send({ aw: 1, type: 'cancel' }, { origin, source: null })
    expect(callOp).not.toHaveBeenCalled()
    expect(onCancel).not.toHaveBeenCalled()

    // Sizes are kept within bounds.
    send({ aw: 1, type: 'resize', height: 99999 }, { origin, source: app })
    expect(frameElement).toHaveStyle({ height: '2000px' })

    // Done asks Workspace for the result instead of trusting the message.
    callOp.mockResolvedValue({
      answer: { kind: 'done', message: 'Keys saved.', result: { saved: true } },
    })
    send({ aw: 1, type: 'done', ref: 'saved-1' }, { origin, source: app })
    expect(callOp).toHaveBeenCalledWith(
      'module_frame_check',
      { launchId: frame.launchId, ref: 'saved-1' },
      expect.objectContaining({ requestId: expect.any(String) }),
    )
    await waitFor(() =>
      expect(onAnswer).toHaveBeenCalledWith({
        kind: 'done',
        message: 'Keys saved.',
        result: { saved: true },
      }),
    )

    send({ aw: 1, type: 'cancel' }, { origin, source: app })
    expect(onCancel).toHaveBeenCalledTimes(1)
  })

  it('shows what went wrong when the app refuses the result, and closes', async () => {
    const onCancel = vi.fn()
    render(<FrameHost frame={frame} onAnswer={vi.fn()} onCancel={onCancel} />)
    const app = appWindow()
    callOp.mockRejectedValue(new Error('The app did not confirm it.'))
    send({ aw: 1, type: 'done', ref: 'nope' }, { origin, source: app })
    expect(await screen.findByText('The app did not confirm it.')).toBeVisible()
    expect(screen.getByText('From keys.example.com')).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Close' }))
    expect(onCancel).toHaveBeenCalledTimes(1)
  })
})
