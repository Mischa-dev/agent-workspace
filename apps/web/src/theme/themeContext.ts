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
  /** The preset in effect after resolving "system". */
  preset: ThemePreset
  setTheme: (theme: ThemeChoice) => void
  setOverrides: (overrides: Record<string, string>) => void
  setDensity: (density: DensityPreference) => void
  setTextSize: (textSize: TextSizePreference) => void
  resetAppearance: () => void
}

export const ThemeContext = createContext<ThemeContextValue>({
  ...DEFAULT_APPEARANCE,
  preset: 'indigo',
  setTheme: () => undefined,
  setOverrides: () => undefined,
  setDensity: () => undefined,
  setTextSize: () => undefined,
  resetAppearance: () => undefined,
})

export function useTheme() {
  return useContext(ThemeContext)
}
