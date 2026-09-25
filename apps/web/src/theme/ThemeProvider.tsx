import { useEffect, useMemo, useState, type ReactNode } from 'react'

import {
  DEFAULT_APPEARANCE,
  readAppearancePreferences,
  resolvePreset,
  writeAppearancePreferences,
  type AppearancePreferences,
} from './preferences'
import { ALL_SLOTS, DERIVED_SLOTS, themeProperties } from './slots'
import { ThemeContext, type ThemeContextValue } from './themeContext'

function prefersDark() {
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ?? false
}

/** Applies the person's theme to <html>. public/theme-boot.js does the same before first paint. */
export function ThemeProvider({ children }: { children: ReactNode }) {
  const [preferences, setPreferences] = useState(readAppearancePreferences)
  const [dark, setDark] = useState(prefersDark)
  const preset = resolvePreset(preferences.theme, dark)

  useEffect(() => {
    const query = window.matchMedia?.('(prefers-color-scheme: dark)')
    if (!query) return undefined
    const update = () => setDark(query.matches)
    query.addEventListener?.('change', update)
    return () => query.removeEventListener?.('change', update)
  }, [])

  useEffect(() => {
    const root = document.documentElement
    root.dataset.theme = preset
    root.dataset.themePreference = preferences.theme
    root.dataset.density = preferences.density
    root.dataset.textSize = preferences.textSize
    for (const slot of new Set([...ALL_SLOTS, ...Object.keys(DERIVED_SLOTS)])) root.style.removeProperty(`--${slot}`)
    for (const [name, value] of Object.entries(themeProperties(preferences.overrides))) root.style.setProperty(name, value)
    const meta = document.querySelector('meta[name="theme-color"]')
    const rail = getComputedStyle(root).getPropertyValue('--rail-bg').trim()
    if (meta && rail) meta.setAttribute('content', rail)
  }, [preferences, preset])

  const value = useMemo<ThemeContextValue>(() => {
    const update = (next: AppearancePreferences) => {
      setPreferences(next)
      try {
        writeAppearancePreferences(next)
      } catch {
        // Appearance persistence is best effort and never blocks the workspace.
      }
    }
    return {
      ...preferences,
      preset,
      setTheme: (theme) => update({ ...preferences, theme, overrides: theme === preferences.theme ? preferences.overrides : {} }),
      setOverrides: (overrides) => update({ ...preferences, overrides }),
      setDensity: (density) => update({ ...preferences, density }),
      setTextSize: (textSize) => update({ ...preferences, textSize }),
      resetAppearance: () => update(DEFAULT_APPEARANCE),
    }
  }, [preferences, preset])

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
}
