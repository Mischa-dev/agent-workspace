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
import { syncThemeColor } from './themeColor'

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
    syncThemeColor()
  }, [preferences, preset])

  const value = useMemo<ThemeContextValue>(() => {
    // Functional updates, so two changes in one event (a pasted theme code
    // sets the preset and then the colors) both land.
    const update = (change: (current: AppearancePreferences) => AppearancePreferences) => {
      setPreferences((current) => {
        const next = change(current)
        try {
          writeAppearancePreferences(next)
        } catch {
          // Appearance persistence is best effort and never blocks the workspace.
        }
        return next
      })
    }
    return {
      ...preferences,
      preset,
      setTheme: (theme) =>
        update((current) => ({ ...current, theme, overrides: theme === current.theme ? current.overrides : {} })),
      setOverrides: (overrides) => update((current) => ({ ...current, overrides })),
      setDensity: (density) => update((current) => ({ ...current, density })),
      setTextSize: (textSize) => update((current) => ({ ...current, textSize })),
      resetAppearance: () => update(() => DEFAULT_APPEARANCE),
      replaceAppearance: (next) => update(() => next),
    }
  }, [preferences, preset])

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
}
