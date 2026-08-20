import { describe, expect, it } from 'vitest'

import {
  APPEARANCE_STORAGE_KEY,
  DEFAULT_APPEARANCE,
  normalizeAppearancePreferences,
  readAppearancePreferences,
  writeAppearancePreferences,
} from './preferences'

describe('appearance preferences', () => {
  it('fails malformed values closed to presentation defaults', () => {
    expect(
      normalizeAppearancePreferences({
        theme: 'credential-dark',
        density: 'microscopic',
        customTheme: {
          schemaVersion: 1,
          accent: 'url(https://attacker.invalid)',
          focus: 'oklch(0.72 0.12 244)',
        },
      }),
    ).toEqual({
      theme: 'system',
      density: 'comfortable',
      customTheme: { schemaVersion: 1, focus: 'oklch(0.72 0.12 244)' },
    })
  })

  it('stores one versioned presentation record and no application state', () => {
    const values = new Map<string, string>()
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
    }
    writeAppearancePreferences(
      { theme: 'light', density: 'compact', customTheme: null },
      storage,
    )
    expect([...values.keys()]).toEqual([APPEARANCE_STORAGE_KEY])
    expect(readAppearancePreferences(storage)).toEqual({
      theme: 'light',
      density: 'compact',
      customTheme: null,
    })
    expect(values.get(APPEARANCE_STORAGE_KEY)).not.toMatch(
      /token|password|credential|actor|project/i,
    )
  })

  it('recovers when storage is unavailable', () => {
    expect(
      readAppearancePreferences({
        getItem: () => {
          throw new Error('disabled')
        },
      }),
    ).toEqual(DEFAULT_APPEARANCE)
  })
})

