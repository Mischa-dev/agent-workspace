import { createContext, useContext } from 'react'

import type {
  AppearancePreferences,
  DensityPreference,
  ThemePreference,
} from './preferences'
import { DEFAULT_APPEARANCE } from './preferences'

export type ResolvedTheme = 'dark' | 'light'

export type ThemeContextValue = AppearancePreferences & {
  resolvedTheme: ResolvedTheme
  setTheme: (theme: ThemePreference) => void
  setDensity: (density: DensityPreference) => void
  resetAppearance: () => void
}

export const ThemeContext = createContext<ThemeContextValue>({
  ...DEFAULT_APPEARANCE,
  resolvedTheme: 'dark',
  setTheme: () => undefined,
  setDensity: () => undefined,
  resetAppearance: () => undefined,
})

export function useTheme() {
  return useContext(ThemeContext)
}
