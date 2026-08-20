import {
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react'

import {
  DEFAULT_APPEARANCE,
  readAppearancePreferences,
  writeAppearancePreferences,
  type AppearancePreferences,
} from './preferences'
import {
  ThemeContext,
  type ResolvedTheme,
  type ThemeContextValue,
} from './themeContext'

function systemTheme(): ResolvedTheme {
  return window.matchMedia?.('(prefers-color-scheme: light)').matches
    ? 'light'
    : 'dark'
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [preferences, setPreferences] = useState(readAppearancePreferences)
  const [system, setSystem] = useState(systemTheme)
  const resolvedTheme = preferences.theme === 'system' ? system : preferences.theme

  useEffect(() => {
    const query = window.matchMedia?.('(prefers-color-scheme: light)')
    if (!query) return
    const update = () => setSystem(query.matches ? 'light' : 'dark')
    query.addEventListener?.('change', update)
    return () => query.removeEventListener?.('change', update)
  }, [])

  useEffect(() => {
    const root = document.documentElement
    root.dataset.theme = resolvedTheme
    root.dataset.themePreference = preferences.theme
    root.dataset.density = preferences.density
    root.style.colorScheme = resolvedTheme

    const customProperties = {
      '--color-accent': preferences.customTheme?.accent,
      '--color-accent-strong': preferences.customTheme?.accentStrong,
      '--color-accent-ink': preferences.customTheme?.accentInk,
      '--color-focus': preferences.customTheme?.focus,
    }
    for (const [name, value] of Object.entries(customProperties)) {
      if (value) root.style.setProperty(name, value)
      else root.style.removeProperty(name)
    }
  }, [preferences, resolvedTheme])

  const update = (next: AppearancePreferences) => {
    const safe = { ...next }
    setPreferences(safe)
    try {
      writeAppearancePreferences(safe)
    } catch {
      // Presentation persistence is best effort and never blocks the workspace.
    }
  }

  const value = useMemo<ThemeContextValue>(
    () => ({
      ...preferences,
      resolvedTheme,
      setTheme: (theme) => update({ ...preferences, theme }),
      setDensity: (density) => update({ ...preferences, density }),
      resetAppearance: () => update(DEFAULT_APPEARANCE),
    }),
    [preferences, resolvedTheme],
  )

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
}
