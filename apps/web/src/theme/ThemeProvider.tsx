import { useEffect, useMemo, useState, type ReactNode } from 'react'

import {
  DEFAULT_APPEARANCE,
  readAppearancePreferences,
  resolvePreset,
  writeAppearancePreferences,
  type AppearancePreferences,
} from './preferences'
import { ALL_SLOTS, DERIVED_SLOTS, isClassicPreset, loadClassicThemes, themeProperties, type ThemePreset } from './slots'
import { ThemeContext, type ThemeContextValue } from './themeContext'
import { syncThemeColor } from './themeColor'

const NO_OVERRIDES: Record<string, string> = {}

function prefersDark() {
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ?? false
}

/** Applies the person's theme to <html>. public/theme-boot.js does the same before first paint. */
export function ThemeProvider({ children }: { children: ReactNode }) {
  const [preferences, setPreferences] = useState(readAppearancePreferences)
  const [dark, setDark] = useState(prefersDark)
  const [previewPreset, setPreviewPreset] = useState<ThemePreset | null>(null)
  const preset = resolvePreset(preferences.theme, dark)
  // A preview shows the preset as it ships; the saved choice and its custom colors stay as they are.
  const shownPreset = previewPreset ?? preset
  const shownOverrides = previewPreset ? NO_OVERRIDES : preferences.overrides

  useEffect(() => {
    const query = window.matchMedia?.('(prefers-color-scheme: dark)')
    if (!query) return undefined
    const update = () => setDark(query.matches)
    query.addEventListener?.('change', update)
    return () => query.removeEventListener?.('change', update)
  }, [])

  useEffect(() => {
    const root = document.documentElement
    if (isClassicPreset(shownPreset)) loadClassicThemes(syncThemeColor)
    root.dataset.theme = shownPreset
    root.dataset.themePreference = preferences.theme
    root.dataset.density = preferences.density
    root.dataset.textSize = preferences.textSize
    for (const slot of new Set([...ALL_SLOTS, ...Object.keys(DERIVED_SLOTS)])) root.style.removeProperty(`--${slot}`)
    for (const [name, value] of Object.entries(themeProperties(shownOverrides))) root.style.setProperty(name, value)
    syncThemeColor()
  }, [preferences, shownPreset, shownOverrides])

  // The setters never change, so effects that depend on them (AppearanceSync
  // applying the account's copy) don't re-run, and undo a choice, on every change.
  const actions = useMemo(() => {
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
      previewTheme: setPreviewPreset,
      setTheme: (theme: AppearancePreferences['theme']) =>
        update((current) => ({ ...current, theme, overrides: theme === current.theme ? current.overrides : {} })),
      setOverrides: (overrides: AppearancePreferences['overrides']) => update((current) => ({ ...current, overrides })),
      setDensity: (density: AppearancePreferences['density']) => update((current) => ({ ...current, density })),
      setTextSize: (textSize: AppearancePreferences['textSize']) => update((current) => ({ ...current, textSize })),
      resetAppearance: () => update(() => DEFAULT_APPEARANCE),
      replaceAppearance: (next: AppearancePreferences) => update(() => next),
    }
  }, [])
  const value = useMemo<ThemeContextValue>(() => ({ ...preferences, preset, previewPreset, ...actions }), [preferences, preset, previewPreset, actions])

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
}
