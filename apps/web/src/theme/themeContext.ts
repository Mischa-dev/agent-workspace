import { createContext, useContext } from 'react'

import type { ThemeDocument } from '../api/generated/operations'
import type { CachedTheme, ModeName } from './customTheme'
import {
  DEFAULT_APPEARANCE,
  type AppearancePreferences,
  type DensityPreference,
  type ModePreference,
  type TextSizePreference,
  type ThemeChoice,
} from './preferences'
import type { ThemePreset } from './slots'

export type ThemeContextValue = AppearancePreferences & {
  /** The person's preset after resolving "system". */
  preset: ThemePreset
  /** A preset shown on screen without being saved, such as one browsed on the design system page. */
  previewPreset: ThemePreset | null
  /** Shows a preset as it ships, without saving it or touching custom colors; null goes back to the person's own theme. */
  previewTheme: (preset: ThemePreset | null) => void
  /** The built-in theme on screen: the preset, a previewed one, or the one a custom theme starts from. */
  shownPreset: ThemePreset
  /** The half of a custom theme on screen, after resolving "system". */
  shownMode: ModeName
  /** The custom theme in use, as this browser has it; null shows the built-in `preset`. */
  customDocument: CachedTheme | null
  /** Picks a built-in theme (and stops using a custom one). */
  setTheme: (theme: ThemeChoice) => void
  /** Picks a custom theme, or stops using one. */
  setCustomTheme: (theme: CachedTheme | null) => void
  /** A newer copy of the custom theme in use, such as one edited on another device. */
  refreshCustomTheme: (theme: CachedTheme) => void
  setMode: (mode: ModePreference) => void
  /** Shows a theme document without saving it (the editor's live preview), in one mode or the device's; null stops. */
  previewDocument: (document: ThemeDocument | null, mode?: ModeName) => void
  setOverrides: (overrides: Record<string, string>) => void
  setDensity: (density: DensityPreference) => void
  setTextSize: (textSize: TextSizePreference) => void
  /** Null goes back to the standard width. */
  setSidebarWidth: (width: number | null) => void
  setPanelWidth: (width: number | null) => void
  resetAppearance: () => void
  /** Take a whole saved appearance, such as the one stored on the account. */
  replaceAppearance: (preferences: AppearancePreferences) => void
}

export const ThemeContext = createContext<ThemeContextValue>({
  ...DEFAULT_APPEARANCE,
  preset: 'quiet',
  previewPreset: null,
  previewTheme: () => undefined,
  shownPreset: 'quiet',
  shownMode: 'light',
  customDocument: null,
  setTheme: () => undefined,
  setCustomTheme: () => undefined,
  refreshCustomTheme: () => undefined,
  setMode: () => undefined,
  previewDocument: () => undefined,
  setOverrides: () => undefined,
  setDensity: () => undefined,
  setTextSize: () => undefined,
  setSidebarWidth: () => undefined,
  setPanelWidth: () => undefined,
  resetAppearance: () => undefined,
  replaceAppearance: () => undefined,
})

export function useTheme() {
  return useContext(ThemeContext)
}
