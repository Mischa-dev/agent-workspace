import { describe, expect, it } from 'vitest'

import {
  APPEARANCE_STORAGE_KEY,
  DEFAULT_APPEARANCE,
  normalizeAppearancePreferences,
  readAppearancePreferences,
  resolvePreset,
  writeAppearancePreferences,
} from './preferences'
import { decodeTheme, encodeTheme } from './slotEditing'
import { themeProperties } from './slots'

function memoryStorage(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial))
  return {
    values,
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, value),
  }
}

describe('appearance preferences', () => {
  it('fails malformed values closed and never lets anything but hex reach CSS', () => {
    expect(
      normalizeAppearancePreferences({
        theme: 'credential-dark',
        density: 'microscopic',
        textSize: 'huge',
        overrides: { 'sidebar-bg': 'url(https://attacker.invalid)', accent: '#AABBCC', 'not-a-slot': '#000000' },
      }),
    ).toEqual({ theme: 'system', density: 'comfortable', textSize: 'default', overrides: { accent: '#aabbcc' }, sidebarWidth: null, panelWidth: null })
  })

  it('keeps dragged widths inside their limits and drops anything that is not a width', () => {
    expect(normalizeAppearancePreferences({ sidebarWidth: 4000, panelWidth: 12.4 })).toMatchObject({ sidebarWidth: 480, panelWidth: 320 })
    expect(normalizeAppearancePreferences({ sidebarWidth: '300px', panelWidth: Number.NaN })).toMatchObject({ sidebarWidth: null, panelWidth: null })
    expect(normalizeAppearancePreferences({ sidebarWidth: 301.6 })).toMatchObject({ sidebarWidth: 302 })
  })

  it('stores one versioned presentation record and no application state', () => {
    const storage = memoryStorage()
    writeAppearancePreferences(
      { theme: 'evergreen', density: 'compact', textSize: 'large', overrides: { accent: '#224466' }, sidebarWidth: 300, panelWidth: null },
      storage,
    )
    expect([...storage.values.keys()]).toEqual([APPEARANCE_STORAGE_KEY])
    expect(readAppearancePreferences(storage)).toEqual({
      theme: 'evergreen',
      density: 'compact',
      textSize: 'large',
      overrides: { accent: '#224466' },
      sidebarWidth: 300,
      panelWidth: null,
    })
    expect(storage.values.get(APPEARANCE_STORAGE_KEY)).not.toMatch(/token|password|credential|actor|project/i)
  })

  it('follows the device until someone picks a theme, and keeps a theme they picked', () => {
    expect(readAppearancePreferences(memoryStorage())).toMatchObject({ theme: 'system' })
    const picked = memoryStorage({ [APPEARANCE_STORAGE_KEY]: '{"theme":"indigo"}' })
    expect(readAppearancePreferences(picked)).toMatchObject({ theme: 'indigo' })
  })

  it('carries a pre-redesign dark choice forward and recovers when storage is unavailable', () => {
    const legacy = memoryStorage({ 'agent-workspace:appearance:v1': '{"theme":"dark","density":"compact"}' })
    expect(readAppearancePreferences(legacy)).toMatchObject({ theme: 'midnight', density: 'compact' })
    const light = memoryStorage({ 'agent-workspace:appearance:v1': '{"theme":"light"}' })
    expect(readAppearancePreferences(light)).toMatchObject({ theme: 'indigo' })
    expect(readAppearancePreferences({ getItem: () => { throw new Error('disabled') } })).toEqual(DEFAULT_APPEARANCE)
    expect(resolvePreset('system', true)).toBe('midnight')
    expect(resolvePreset('system', false)).toBe('daylight')
  })

  it('round-trips theme codes and derives follower slots from changed bases', () => {
    const code = encodeTheme('graphite', { 'sidebar-bg': '#123456' })
    expect(decodeTheme(code)).toEqual({ preset: 'graphite', overrides: { 'sidebar-bg': '#123456' } })
    expect(() => decodeTheme('hello')).toThrow(/AW1/)
    const properties = themeProperties({ 'sidebar-bg': '#123456' })
    expect(properties['--sidebar-bg']).toBe('#123456')
    expect(properties['--sidebar-hover']).toContain('var(--sidebar-bg)')
    expect(properties['--accent-hover']).toBeUndefined()
  })
})
