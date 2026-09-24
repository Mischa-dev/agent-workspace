export const APPEARANCE_STORAGE_KEY = 'agent-workspace:appearance:v1'

export type ThemePreference = 'system' | 'dark' | 'light'
export type DensityPreference = 'compact' | 'comfortable' | 'spacious'

export type CustomTheme = {
  schemaVersion: 1
  accent?: string
  accentStrong?: string
  accentInk?: string
  focus?: string
}

export type AppearancePreferences = {
  theme: ThemePreference
  density: DensityPreference
  customTheme: CustomTheme | null
}

export const DEFAULT_APPEARANCE: AppearancePreferences = {
  theme: 'system',
  density: 'comfortable',
  customTheme: null,
}

const colorPattern = /^(#[\da-f]{3,8}|(?:oklch|rgb|rgba|hsl|hsla)\([\d.%+\-/,\s]+\))$/i

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {}
}

function safeColor(value: unknown) {
  if (typeof value !== 'string' || value.length > 96) return undefined
  const normalized = value.trim()
  return colorPattern.test(normalized) ? normalized : undefined
}

export function normalizeCustomTheme(value: unknown): CustomTheme | null {
  const candidate = record(value)
  if (candidate.schemaVersion !== 1) return null
  const customTheme: CustomTheme = { schemaVersion: 1 }
  const accent = safeColor(candidate.accent)
  const accentStrong = safeColor(candidate.accentStrong)
  const accentInk = safeColor(candidate.accentInk)
  const focus = safeColor(candidate.focus)
  if (accent) customTheme.accent = accent
  if (accentStrong) customTheme.accentStrong = accentStrong
  if (accentInk) customTheme.accentInk = accentInk
  if (focus) customTheme.focus = focus
  return Object.keys(customTheme).length > 1 ? customTheme : null
}

export function normalizeAppearancePreferences(
  value: unknown,
): AppearancePreferences {
  const candidate = record(value)
  const theme: ThemePreference =
    candidate.theme === 'dark' ||
    candidate.theme === 'light' ||
    candidate.theme === 'system'
      ? candidate.theme
      : DEFAULT_APPEARANCE.theme
  const density: DensityPreference =
    candidate.density === 'compact' ||
    candidate.density === 'comfortable' ||
    candidate.density === 'spacious'
      ? candidate.density
      : DEFAULT_APPEARANCE.density
  return {
    theme,
    density,
    customTheme: normalizeCustomTheme(candidate.customTheme),
  }
}

export function readAppearancePreferences(
  storage: Pick<Storage, 'getItem'> = window.localStorage,
) {
  try {
    const stored = storage.getItem(APPEARANCE_STORAGE_KEY)
    return stored
      ? normalizeAppearancePreferences(JSON.parse(stored) as unknown)
      : DEFAULT_APPEARANCE
  } catch {
    return DEFAULT_APPEARANCE
  }
}

export function writeAppearancePreferences(
  preferences: AppearancePreferences,
  storage: Pick<Storage, 'setItem'> = window.localStorage,
) {
  storage.setItem(
    APPEARANCE_STORAGE_KEY,
    JSON.stringify(normalizeAppearancePreferences(preferences)),
  )
}
