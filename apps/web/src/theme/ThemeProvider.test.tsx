import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import AppearanceSync from './AppearanceSync'
import { THEME_DOCUMENT_STORAGE_KEY, THEME_STYLE_ID, writeCachedTheme } from './customTheme'
import { APPEARANCE_STORAGE_KEY, DEFAULT_APPEARANCE, writeAppearancePreferences } from './preferences'
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
  document.getElementById(THEME_STYLE_ID)?.remove()
})

describe('ThemeProvider preview', () => {
  it('shows a preset without saving it or dropping custom colors, here or on the account', async () => {
    const saved = {
      theme: 'indigo' as const,
      customTheme: null,
      mode: 'system' as const,
      overrides: { accent: '#ff0066' },
      density: 'comfortable' as const,
      textSize: 'default' as const,
      sidebarWidth: null,
      panelWidth: 560,
    }
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

describe('ThemeProvider custom themes', () => {
  const ids = { id: '0193f0a0-0000-7000-8000-000000000001', workspaceId: '0193f0a0-0000-7000-8000-000000000002' }

  it('paints the cached theme in the chosen mode: its base, its tokens and its CSS last in <head>', () => {
    writeAppearancePreferences({ ...DEFAULT_APPEARANCE, customTheme: ids, mode: 'dark' })
    writeCachedTheme({
      ...ids,
      version: 3,
      name: 'Harbour',
      document: {
        format: 1,
        light: { base: 'quiet', tokens: { accent: '#224466' } },
        dark: { base: 'paper-dark', tokens: { accent: '#88aaff', 'radius-md': '2px', 'content-bg': 'url(https://x.invalid/a.png)' } },
        css: '.aw-card { border-color: var(--accent); }',
      },
    })
    render(
      <ThemeProvider>
        <span />
      </ThemeProvider>,
    )
    const root = document.documentElement
    expect(root.dataset.theme).toBe('paper-dark')
    expect(root.style.getPropertyValue('--accent')).toBe('#88aaff')
    expect(root.style.getPropertyValue('--radius-md')).toBe('2px')
    // A cache edited by hand never reaches CSS with a url().
    expect(root.style.getPropertyValue('--content-bg')).toBe('')
    expect(root.style.getPropertyValue('--accent-hover')).toContain('var(--accent)')
    const style = document.getElementById(THEME_STYLE_ID)
    expect(style?.textContent).toContain('.aw-card')
    expect(document.head.lastElementChild).toBe(style)
  })

  it('falls back to the built-in theme when the cached document is for another theme', () => {
    writeAppearancePreferences({ ...DEFAULT_APPEARANCE, theme: 'evergreen', customTheme: ids })
    window.localStorage.setItem(THEME_DOCUMENT_STORAGE_KEY, JSON.stringify({ id: 'other', workspaceId: ids.workspaceId, document: { light: {}, dark: {} } }))
    render(
      <ThemeProvider>
        <span />
      </ThemeProvider>,
    )
    expect(document.documentElement.dataset.theme).toBe('evergreen')
    expect(document.getElementById(THEME_STYLE_ID)).toBeNull()
  })
})
