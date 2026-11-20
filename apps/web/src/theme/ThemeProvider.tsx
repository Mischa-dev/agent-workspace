import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'

import type { ThemeDocument } from '../api/generated/operations'
import {
  applyThemeCss,
  documentBase,
  documentCss,
  documentProperties,
  readCachedTheme,
  writeCachedTheme,
  type CachedTheme,
  type ModeName,
} from './customTheme'
import {
  DEFAULT_APPEARANCE,
  PANEL_WIDTHS,
  SIDEBAR_WIDTHS,
  clampWidth,
  readAppearancePreferences,
  resolveMode,
  resolvePreset,
  writeAppearancePreferences,
  type AppearancePreferences,
} from './preferences'
import { loadThemeSheets, presetSheet, themeProperties, type ThemePreset } from './slots'
import { ThemeContext, type ThemeContextValue } from './themeContext'
import { syncThemeColor } from './themeColor'

function prefersDark() {
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ?? false
}

function bootProperties(): string[] {
  const root = document.documentElement
  const names = (root.dataset.themeProperties ?? '').split(' ').filter((name) => name.startsWith('--'))
  delete root.dataset.themeProperties
  return names
}

/** The cached custom theme, when it's the one the preferences pick. */
function cachedFor(preferences: AppearancePreferences): CachedTheme | null {
  const cached = preferences.customTheme ? readCachedTheme() : null
  return cached && cached.id === preferences.customTheme?.id ? cached : null
}

/** Applies the person's theme to <html>. public/theme-boot.js does the same before first paint. */
export function ThemeProvider({ children }: { children: ReactNode }) {
  const [preferences, setPreferences] = useState(readAppearancePreferences)
  const [customDocument, setCustomDocument] = useState(() => cachedFor(readAppearancePreferences()))
  const [dark, setDark] = useState(prefersDark)
  const [previewPreset, setPreviewPreset] = useState<ThemePreset | null>(null)
  const [previewed, setPreviewed] = useState<{ document: ThemeDocument; mode?: ModeName } | null>(null)
  // The properties on <html> that this theme set; theme-boot.js lists its own in data-theme-properties.
  const [booted] = useState(bootProperties)
  const applied = useRef<string[]>(booted)
  const preset = resolvePreset(preferences.theme, dark)
  const shownMode = previewed?.mode ?? resolveMode(preferences.mode, dark)

  // What shows: a previewed preset as it ships, else a previewed or chosen
  // custom document, else the chosen preset with its old colour overrides.
  const shown = useMemo(() => {
    if (previewPreset) return { preset: previewPreset, properties: {}, css: '' }
    const document = previewed?.document ?? (preferences.customTheme ? customDocument?.document : null)
    if (document) return { preset: documentBase(document, shownMode), properties: documentProperties(document, shownMode), css: documentCss(document) }
    return { preset, properties: themeProperties(preferences.overrides), css: '' }
  }, [previewPreset, previewed, preferences.customTheme, preferences.overrides, customDocument, shownMode, preset])

  useEffect(() => {
    const query = window.matchMedia?.('(prefers-color-scheme: dark)')
    if (!query) return undefined
    const update = () => setDark(query.matches)
    query.addEventListener?.('change', update)
    return () => query.removeEventListener?.('change', update)
  }, [])

  useEffect(() => {
    const root = document.documentElement
    loadThemeSheets(presetSheet(shown.preset), syncThemeColor)
    root.dataset.theme = shown.preset
    root.dataset.themePreference = preferences.customTheme ? 'custom' : preferences.theme
    root.dataset.density = preferences.density
    root.dataset.textSize = preferences.textSize
    for (const name of applied.current) root.style.removeProperty(name)
    applied.current = Object.keys(shown.properties)
    for (const [name, value] of Object.entries(shown.properties)) root.style.setProperty(name, value)
    applyThemeCss(shown.css)
    for (const [name, width] of [
      ['--sidebar-width', preferences.sidebarWidth],
      ['--panel-width', preferences.panelWidth],
    ] as const) {
      if (width === null) root.style.removeProperty(name)
      else root.style.setProperty(name, `${width}px`)
    }
    syncThemeColor()
  }, [preferences, shown])

  // The setters never change, so effects that depend on them (AppearanceSync
  // applying the account's copy) don't re-run, and undo a choice, on every change.
  const actions = useMemo(() => {
    // Functional updates, so two changes in one event both land.
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
    const cache = (theme: CachedTheme | null) => {
      writeCachedTheme(theme)
      setCustomDocument(theme)
    }
    return {
      previewTheme: setPreviewPreset,
      previewDocument: (document: ThemeDocument | null, mode?: ModeName) => setPreviewed(document ? { document, mode } : null),
      setTheme: (theme: AppearancePreferences['theme']) => {
        cache(null)
        update((current) => ({ ...current, theme, customTheme: null, overrides: {} }))
      },
      setCustomTheme: (theme: CachedTheme | null) => {
        cache(theme)
        update((current) => ({ ...current, customTheme: theme ? { id: theme.id, workspaceId: theme.workspaceId } : null, overrides: {} }))
      },
      refreshCustomTheme: (theme: CachedTheme) => {
        setCustomDocument((current) => {
          if (current && current.id !== theme.id) return current
          writeCachedTheme(theme)
          return theme
        })
      },
      setMode: (mode: AppearancePreferences['mode']) => update((current) => ({ ...current, mode })),
      setOverrides: (overrides: AppearancePreferences['overrides']) => update((current) => ({ ...current, overrides })),
      setDensity: (density: AppearancePreferences['density']) => update((current) => ({ ...current, density })),
      setTextSize: (textSize: AppearancePreferences['textSize']) => update((current) => ({ ...current, textSize })),
      setSidebarWidth: (width: number | null) => update((current) => ({ ...current, sidebarWidth: clampWidth(width, SIDEBAR_WIDTHS) })),
      setPanelWidth: (width: number | null) => update((current) => ({ ...current, panelWidth: clampWidth(width, PANEL_WIDTHS) })),
      resetAppearance: () => {
        cache(null)
        update(() => DEFAULT_APPEARANCE)
      },
      replaceAppearance: (next: AppearancePreferences) => {
        // The account picked a different custom theme: drop the cached one until CustomThemeSync reads it.
        setCustomDocument((current) => (current && current.id === next.customTheme?.id ? current : cachedFor(next)))
        update(() => next)
      },
    }
  }, [])
  const value = useMemo<ThemeContextValue>(
    () => ({
      ...preferences,
      preset,
      previewPreset,
      shownPreset: shown.preset,
      shownMode,
      customDocument: preferences.customTheme ? customDocument : null,
      ...actions,
    }),
    [preferences, preset, previewPreset, shown.preset, shownMode, customDocument, actions],
  )

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
}
