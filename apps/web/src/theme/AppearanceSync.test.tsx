import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import AppearanceSync from './AppearanceSync'
import { ThemeProvider } from './ThemeProvider'
import { useTheme } from './themeContext'

function Picker() {
  const theme = useTheme()
  return (
    <button onClick={() => theme.setTheme('midnight')} type="button">
      {theme.theme}
    </button>
  )
}

afterEach(() => {
  vi.unstubAllGlobals()
  window.localStorage.clear()
})

describe('AppearanceSync', () => {
  it('keeps a theme picked after the account copy arrived', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_url: string, init?: RequestInit) =>
        Response.json(
          init?.method === 'PUT'
            ? { appearance: JSON.parse(String(init.body)).appearance, version: 2 }
            : { appearance: { theme: 'indigo', overrides: {}, density: 'comfortable', textSize: 'default' }, version: 1 },
        ),
      ),
    )
    render(
      <QueryClientProvider client={new QueryClient()}>
        <ThemeProvider>
          <AppearanceSync />
          <Picker />
        </ThemeProvider>
      </QueryClientProvider>,
    )
    await waitFor(() => expect(fetch).toHaveBeenCalled())
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })

    act(() => screen.getByRole('button').click())

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50))
    })
    expect(screen.getByRole('button')).toHaveTextContent('midnight')
    expect(document.documentElement.dataset.theme).toBe('midnight')
  })
})
