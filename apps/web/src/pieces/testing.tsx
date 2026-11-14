import { render, type RenderResult } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import axe from 'axe-core'
import type { ReactElement } from 'react'
import { MemoryRouter } from 'react-router-dom'
import { vi } from 'vitest'

/** The widths a piece is checked at: a desk and a phone. */
export const WIDTHS = { wide: 1280, narrow: 390 } as const
export type Width = keyof typeof WIDTHS

/**
 * Make `matchMedia` answer as a window of this width would (with a coarse
 * pointer on the phone), so `usePhoneWidth` and `useCoarsePointer` pick the
 * matching form. jsdom lays nothing out: real widths are checked in e2e.
 */
export function atWidth(width: Width) {
  const pixels = WIDTHS[width]
  vi.stubGlobal('matchMedia', (query: string) => {
    const max = /max-width:\s*(\d+)px/.exec(query)
    const min = /min-width:\s*(\d+)px/.exec(query)
    const matches = max
      ? pixels <= Number(max[1])
      : min
        ? pixels >= Number(min[1])
        : query.includes('pointer: coarse')
          ? width === 'narrow'
          : false
    return {
      matches,
      media: query,
      onchange: null,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      addListener: () => undefined,
      removeListener: () => undefined,
      dispatchEvent: () => false,
    }
  })
}

/** Render a piece with what pieces read around them: a router and a query client. */
export function renderPiece(ui: ReactElement, { width = 'wide' }: { width?: Width } = {}): RenderResult {
  atWidth(width)
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>{ui}</MemoryRouter>
    </QueryClientProvider>,
  )
}

/** Accessibility problems axe finds (contrast needs a real browser). */
export async function axeViolations(container: HTMLElement) {
  return (await axe.run(container, { rules: { 'color-contrast': { enabled: false } } })).violations
}
