// The account's saved appearance, read by AppearanceSync (which keeps it and
// this browser in step) and CustomThemeSync (which waits for it before
// turning old colour overrides into a theme). One query key, so both share
// one cached copy: AppearanceSync fetches it, CustomThemeSync only reads it.

export type StoredAppearance = { appearance: unknown; version: number }

export const appearanceQueryKey = ['account', 'appearance'] as const
