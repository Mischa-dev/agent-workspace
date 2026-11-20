import { isThemePreset, sanitizeOverrides, type ThemePreset } from './slots'

export const APPEARANCE_STORAGE_KEY = 'agent-workspace:appearance:v2'
const LEGACY_STORAGE_KEY = 'agent-workspace:appearance:v1'

/** A preset, or "system": Quiet, light or dark as the device is. */
export type ThemeChoice = ThemePreset | 'system'
/** Which half of a custom theme shows: the device's, or always light or dark. */
export type ModePreference = 'system' | 'light' | 'dark'
/** A custom theme someone picked; the document itself is cached (customTheme.ts) and read with theme_get. */
export type CustomThemeChoice = { id: string; workspaceId: string }
export type DensityPreference = 'comfortable' | 'compact'
export type TextSizePreference = 'small' | 'default' | 'large'

export type AppearancePreferences = {
  theme: ThemeChoice
  /** A custom theme, shown instead of `theme` while it's set. */
  customTheme: CustomThemeChoice | null
  mode: ModePreference
  /** Colours changed on a built-in theme before custom themes; the app turns them into a custom theme once. */
  overrides: Record<string, string>
  density: DensityPreference
  textSize: TextSizePreference
  /** The sidebar and the side panel as wide as you dragged them, in pixels; null is the standard width. */
  sidebarWidth: number | null
  panelWidth: number | null
}

/** Until someone picks a theme, it's Quiet, following the device between light and dark. */
export const DEFAULT_APPEARANCE: AppearancePreferences = {
  theme: 'system',
  customTheme: null,
  mode: 'system',
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

const ID = /^[0-9a-f-]{36}$/i

function customTheme(value: unknown): CustomThemeChoice | null {
  const found = record(value)
  return typeof found.id === 'string' && ID.test(found.id) && typeof found.workspaceId === 'string' && ID.test(found.workspaceId)
    ? { id: found.id, workspaceId: found.workspaceId }
    : null
}

export function normalizeAppearancePreferences(value: unknown): AppearancePreferences {
  const candidate = record(value)
  return {
    theme: candidate.theme === 'system' || isThemePreset(candidate.theme) ? candidate.theme : DEFAULT_APPEARANCE.theme,
    customTheme: customTheme(candidate.customTheme),
    mode: candidate.mode === 'light' || candidate.mode === 'dark' ? candidate.mode : 'system',
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
  return prefersDark ? 'quiet-dark' : 'quiet'
}

/** The half of a custom theme that shows. */
export function resolveMode(mode: ModePreference, prefersDark: boolean): 'light' | 'dark' {
  if (mode !== 'system') return mode
  return prefersDark ? 'dark' : 'light'
}
