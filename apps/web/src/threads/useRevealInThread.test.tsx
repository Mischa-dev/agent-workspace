import { act, cleanup, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { useRevealInThread } from './useRevealInThread'

type Props = Parameters<typeof useRevealInThread>[0]

const scrollIntoView = vi.fn()

beforeEach(() => {
  scrollIntoView.mockClear()
  Element.prototype.scrollIntoView = scrollIntoView
})

afterEach(() => {
  cleanup()
  document.body.innerHTML = ''
})

function setup(loadOlder = vi.fn(async () => undefined)) {
  const initial: Props = { threadId: 'thread-1', cursor: 'page-1', error: null, loading: false, loadOlder, olderCount: 0 }
  const view = renderHook((props: Props) => useRevealInThread(props), { initialProps: initial })
  let props = initial
  const update = (next: Partial<Props>) => {
    props = { ...props, ...next }
    view.rerender(props)
  }
  return { ...view, loadOlder, update }
}

function showMessage(id: string) {
  const element = document.createElement('div')
  element.id = `message-${id}`
  document.body.append(element)
}

describe('useRevealInThread', () => {
  it('loads earlier pages until the message is there, then scrolls to it and stops', () => {
    const { result, loadOlder, update } = setup()
    act(() => result.current('message-9'))
    expect(loadOlder).toHaveBeenCalledTimes(1)
    update({ loading: true })
    update({ loading: false, cursor: 'page-2', olderCount: 50 })
    expect(loadOlder).toHaveBeenCalledTimes(2)

    update({ loading: true })
    showMessage('message-9')
    update({ loading: false, cursor: 'page-3', olderCount: 100 })
    expect(scrollIntoView).toHaveBeenCalledTimes(1)
    expect(loadOlder).toHaveBeenCalledTimes(2)
    update({ olderCount: 101 })
    expect(loadOlder).toHaveBeenCalledTimes(2)
  })

  it('stops at a page that fails, without loading again', () => {
    const { result, loadOlder, update } = setup()
    act(() => result.current('message-9'))
    update({ loading: true })
    update({ loading: false, error: 'Earlier messages didn’t load.' })
    update({ olderCount: 1 })
    expect(loadOlder).toHaveBeenCalledTimes(1)
    expect(scrollIntoView).not.toHaveBeenCalled()
  })

  it('stops at a page that brings nothing new', () => {
    const { result, loadOlder, update } = setup()
    act(() => result.current('message-9'))
    update({ loading: true })
    update({ loading: false })
    expect(loadOlder).toHaveBeenCalledTimes(1)
  })

  it('drops a lookup still loading when the thread changes or closes', () => {
    const { result, loadOlder, update, unmount } = setup()
    act(() => result.current('message-9'))
    update({ loading: true })
    update({ threadId: 'thread-2', loading: false, cursor: 'page-2', olderCount: 50 })
    update({ cursor: 'page-3' })
    expect(loadOlder).toHaveBeenCalledTimes(1)

    act(() => result.current('message-7'))
    expect(loadOlder).toHaveBeenCalledTimes(2)
    update({ loading: true })
    unmount()
    expect(loadOlder).toHaveBeenCalledTimes(2)
  })

  it('scrolls to a message already shown, and loads nothing when nothing is older', () => {
    const { result, loadOlder, update } = setup()
    showMessage('message-1')
    act(() => result.current('message-1'))
    expect(scrollIntoView).toHaveBeenCalledTimes(1)
    update({ cursor: null })
    act(() => result.current('message-9'))
    expect(loadOlder).not.toHaveBeenCalled()
  })
})
