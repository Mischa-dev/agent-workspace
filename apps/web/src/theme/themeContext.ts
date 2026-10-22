import { createContext, useContext } from 'react'

import {
  DEFAULT_APPEARANCE,
  type AppearancePreferences,
  type DensityPreference,
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
  setTheme: (theme: ThemeChoice) => void
  setOverrides: (overrides: Record<string, string>) => void
  setDensity: (density: DensityPreference) => void
  setTextSize: (textSize: TextSizePreference) => void
  resetAppearance: () => void
  /** Take a whole saved appearance, such as the one stored on the account. */
  replaceAppearance: (preferences: AppearancePreferences) => void
}

export const ThemeContext = createContext<ThemeContextValue>({
  ...DEFAULT_APPEARANCE,
  preset: 'indigo',
  previewPreset: null,
  previewTheme: () => undefined,
  setTheme: () => undefined,
  setOverrides: () => undefined,
  setDensity: () => undefined,
  setTextSize: () => undefined,
  resetAppearance: () => undefined,
  replaceAppearance: () => undefined,
})

export function useTheme() {
  return useContext(ThemeContext)
}
