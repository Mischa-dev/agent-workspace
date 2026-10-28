import { isThemePreset, sanitizeOverrides, type ThemePreset } from './slots'

export const APPEARANCE_STORAGE_KEY = 'agent-workspace:appearance:v2'
const LEGACY_STORAGE_KEY = 'agent-workspace:appearance:v1'

/** A preset, or "system": Indigo in light mode and Midnight in dark mode. */
export type ThemeChoice = ThemePreset | 'system'
export type DensityPreference = 'comfortable' | 'compact'
export type TextSizePreference = 'small' | 'default' | 'large'

export type AppearancePreferences = {
  theme: ThemeChoice
  overrides: Record<string, string>
  density: DensityPreference
  textSize: TextSizePreference
  /** The sidebar and the side panel as wide as you dragged them, in pixels; null is the standard width. */
  sidebarWidth: number | null
  panelWidth: number | null
}

/** Until someone picks a theme, it follows the device: Indigo in light mode, Midnight in dark. */
export const DEFAULT_APPEARANCE: AppearancePreferences = {
  theme: 'system',
  overrides: {},
  density: 'comfortable',
  textSize: 'default',
  sidebarWidth: null,
  panelWidth: null,
}

export type WidthLimits = { min: number; standard: number; max: number }

/** How narrow and wide the edges drag. public/theme-boot.js and styles/shell.css hold the same numbers. */
export const SIDEBAR_WIDTHS: WidthLimits = { min: 200, standard: 264, max: 480 }
export const PANEL_WIDTHS: WidthLimits = { min: 320, standard: 420, max: 800 }

export function clampWidth(value: unknown, limits: WidthLimits): number | null {
  if (typeof value !== 'number' || !Number.isFinite(value)) return null
  return Math.round(Math.min(limits.max, Math.max(limits.min, value)))
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {}
}

export function normalizeAppearancePreferences(value: unknown): AppearancePreferences {
  const candidate = record(value)
  return {
    theme: candidate.theme === 'system' || isThemePreset(candidate.theme) ? candidate.theme : DEFAULT_APPEARANCE.theme,
    overrides: sanitizeOverrides(candidate.overrides),
    density: candidate.density === 'compact' ? 'compact' : 'comfortable',
    textSize: candidate.textSize === 'small' || candidate.textSize === 'large' ? candidate.textSize : 'default',
    sidebarWidth: clampWidth(candidate.sidebarWidth, SIDEBAR_WIDTHS),
    panelWidth: clampWidth(candidate.panelWidth, PANEL_WIDTHS),
  }
}

/** Carry a pre-redesign choice forward: dark stays dark, light stays light, anything else follows the device. */
function migrateLegacy(value: unknown): AppearancePreferences {
  const legacy = record(value)
  return {
    ...DEFAULT_APPEARANCE,
    theme: legacy.theme === 'dark' ? 'midnight' : legacy.theme === 'light' ? 'indigo' : DEFAULT_APPEARANCE.theme,
    density: legacy.density === 'compact' ? 'compact' : 'comfortable',
  }
}

export function readAppearancePreferences(storage: Pick<Storage, 'getItem'> = window.localStorage): AppearancePreferences {
  try {
    const stored = storage.getItem(APPEARANCE_STORAGE_KEY)
    if (stored) return normalizeAppearancePreferences(JSON.parse(stored) as unknown)
    const legacy = storage.getItem(LEGACY_STORAGE_KEY)
    return legacy ? migrateLegacy(JSON.parse(legacy) as unknown) : DEFAULT_APPEARANCE
  } catch {
    return DEFAULT_APPEARANCE
  }
}

export function writeAppearancePreferences(
  preferences: AppearancePreferences,
  storage: Pick<Storage, 'setItem'> = window.localStorage,
) {
  storage.setItem(APPEARANCE_STORAGE_KEY, JSON.stringify(normalizeAppearancePreferences(preferences)))
}

export function resolvePreset(theme: ThemeChoice, prefersDark: boolean): ThemePreset {
  if (theme !== 'system') return theme
  return prefersDark ? 'midnight' : 'indigo'
}
