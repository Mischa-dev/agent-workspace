import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { AppErrorBoundary } from './AppErrorBoundary'
import { failingComponent } from './failingComponent'

const reportError = vi.hoisted(() => vi.fn())
vi.mock('../telemetry/reportErrors', () => ({ reportError }))

let broken = true

function Flaky() {
  if (broken) {
    throw new Error('Cannot read properties of undefined')
  }
  return <p>Back to work</p>
}

afterEach(() => {
  cleanup()
  reportError.mockReset()
  vi.restoreAllMocks()
  broken = true
})

describe('AppErrorBoundary', () => {
  it('reports the error and recovers on Try again', () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined)
    render(
      <AppErrorBoundary>
        <Flaky />
      </AppErrorBoundary>,
    )

    expect(screen.getByRole('alert')).toHaveTextContent('Something went wrong on this page')
    expect(reportError).toHaveBeenCalledTimes(1)
    expect(reportError).toHaveBeenCalledWith('Cannot read properties of undefined', 'react:Flaky')

    broken = false
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(screen.getByText('Back to work')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Reload workspace' })).toBeNull()
  })

  it('clears the error when the address changes, so Back works again', () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined)
    const { rerender } = render(
      <AppErrorBoundary resetKey="/broken">
        <Flaky />
      </AppErrorBoundary>,
    )
    expect(screen.getByRole('alert')).toHaveTextContent('Something went wrong on this page')

    broken = false
    rerender(
      <AppErrorBoundary resetKey="/home">
        <Flaky />
      </AppErrorBoundary>,
    )
    expect(screen.getByText('Back to work')).toBeInTheDocument()
  })

  it('names the failing component without URLs', () => {
    expect(failingComponent('\n    at ThreadPage (http://localhost:5173/src/threads/ThreadPage.tsx:10:3)\n    at div')).toBe(
      'ThreadPage',
    )
    expect(failingComponent('\nThreadPage@http://localhost/x.js:1:1')).toBe('ThreadPage')
    expect(failingComponent(undefined)).toBeUndefined()
  })
})
