import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import AppearanceSync from './AppearanceSync'
import { APPEARANCE_STORAGE_KEY, writeAppearancePreferences } from './preferences'
import { ThemeProvider } from './ThemeProvider'
import { useTheme } from './themeContext'

function Previewer() {
  const theme = useTheme()
  return (
    <>
      <button onClick={() => theme.previewTheme('midnight')} type="button">
        preview
      </button>
      <button onClick={() => theme.previewTheme(null)} type="button">
        mine
      </button>
      <output>{`${theme.theme}:${theme.previewPreset ?? 'none'}`}</output>
    </>
  )
}

afterEach(() => {
  vi.unstubAllGlobals()
  window.localStorage.clear()
  document.documentElement.removeAttribute('style')
})

describe('ThemeProvider preview', () => {
  it('shows a preset without saving it or dropping custom colors, here or on the account', async () => {
    const saved = { theme: 'indigo' as const, overrides: { accent: '#ff0066' }, density: 'comfortable' as const, textSize: 'default' as const }
    writeAppearancePreferences(saved)
    const stored = window.localStorage.getItem(APPEARANCE_STORAGE_KEY)
    const fetch = vi.fn(async (_url: string, init?: RequestInit) =>
      Response.json(init?.method === 'PUT' ? { appearance: JSON.parse(String(init.body)).appearance, version: 2 } : { appearance: saved, version: 1 }),
    )
    vi.stubGlobal('fetch', fetch)
    render(
      <QueryClientProvider client={new QueryClient()}>
        <ThemeProvider>
          <AppearanceSync />
          <Previewer />
        </ThemeProvider>
      </QueryClientProvider>,
    )
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    const root = document.documentElement
    expect(root.style.getPropertyValue('--accent')).toBe('#ff0066')

    act(() => screen.getByRole('button', { name: 'preview' }).click())
    expect(root.dataset.theme).toBe('midnight')
    expect(root.style.getPropertyValue('--accent')).toBe('')
    expect(screen.getByRole('status')).toHaveTextContent('indigo:midnight')

    // Longer than AppearanceSync waits before saving a change.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 1000))
    })
    expect(fetch.mock.calls.some(([, init]) => init?.method === 'PUT')).toBe(false)
    expect(window.localStorage.getItem(APPEARANCE_STORAGE_KEY)).toBe(stored)

    act(() => screen.getByRole('button', { name: 'mine' }).click())
    expect(root.dataset.theme).toBe('indigo')
    expect(root.style.getPropertyValue('--accent')).toBe('#ff0066')
  })
})
