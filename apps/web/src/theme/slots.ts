/** Theme presets and the color slots a person can change. Mirrors the design system's tokens. */

/**
 * The workspace's own presets live in styles/tokens.css. Classic ones (group
 * dark or light) live in public/themes/classic.css, which loads only when one
 * is in use; public/theme-boot.js keeps the same ids.
 */
export const THEME_PRESETS = [
  { id: 'indigo', name: 'Indigo', group: 'workspace' },
  { id: 'midnight', name: 'Midnight', group: 'workspace' },
  { id: 'daylight', name: 'Daylight', group: 'workspace' },
  { id: 'evergreen', name: 'Evergreen', group: 'workspace' },
  { id: 'graphite', name: 'Graphite', group: 'workspace' },
  { id: 'nord', name: 'Nord', group: 'dark' },
  { id: 'dracula', name: 'Dracula', group: 'dark' },
  { id: 'gruvbox-dark', name: 'Gruvbox Dark', group: 'dark' },
  { id: 'solarized-dark', name: 'Solarized Dark', group: 'dark' },
  { id: 'catppuccin-frappe', name: 'Catppuccin Frappé', group: 'dark' },
  { id: 'catppuccin-macchiato', name: 'Catppuccin Macchiato', group: 'dark' },
  { id: 'catppuccin-mocha', name: 'Catppuccin Mocha', group: 'dark' },
  { id: 'tokyo-night', name: 'Tokyo Night', group: 'dark' },
  { id: 'one-dark', name: 'One Dark', group: 'dark' },
  { id: 'rose-pine', name: 'Rosé Pine', group: 'dark' },
  { id: 'github-dark', name: 'GitHub Dark', group: 'dark' },
  { id: 'monokai', name: 'Monokai', group: 'dark' },
  { id: 'everforest-dark', name: 'Everforest Dark', group: 'dark' },
  { id: 'kanagawa', name: 'Kanagawa', group: 'dark' },
  { id: 'gruvbox-light', name: 'Gruvbox Light', group: 'light' },
  { id: 'solarized-light', name: 'Solarized Light', group: 'light' },
  { id: 'catppuccin-latte', name: 'Catppuccin Latte', group: 'light' },
  { id: 'rose-pine-dawn', name: 'Rosé Pine Dawn', group: 'light' },
  { id: 'github-light', name: 'GitHub Light', group: 'light' },
  { id: 'everforest-light', name: 'Everforest Light', group: 'light' },
  { id: 'one-light', name: 'One Light', group: 'light' },
] as const satisfies readonly { id: string; name: string; group: 'workspace' | 'dark' | 'light' }[]

export type ThemePreset = (typeof THEME_PRESETS)[number]['id']

export function isThemePreset(value: unknown): value is ThemePreset {
  return THEME_PRESETS.some((preset) => preset.id === value)
}

/** Where the classic presets' stylesheet is served; public/theme-boot.js uses the same path. */
export const CLASSIC_THEMES_HREF = '/themes/classic.css'

export function isClassicPreset(preset: ThemePreset) {
  return THEME_PRESETS.some((entry) => entry.id === preset && entry.group !== 'workspace')
}

/** Load the classic presets' stylesheet once; theme-boot.js already has when a classic preset was saved. */
export function loadClassicThemes(onLoad?: () => void) {
  if (typeof document === 'undefined' || document.querySelector('link[data-classic-themes]')) return
  const link = document.createElement('link')
  link.rel = 'stylesheet'
  link.href = CLASSIC_THEMES_HREF
  link.dataset.classicThemes = ''
  if (onLoad) link.addEventListener('load', onLoad, { once: true })
  document.head.appendChild(link)
}

/**
 * Every color slot a person can change, in the Appearance page's order. The
 * page's labels and contrast checks are in slotEditing.ts, which loads with
 * it, so the app shell carries only what applying a saved theme needs.
 */
export const ALL_SLOTS: readonly string[] = [
  'rail-bg',
  'rail-fg',
  'rail-tile',
  'rail-tile-active',
  'rail-indicator',
  'sidebar-bg',
  'sidebar-fg',
  'sidebar-fg-strong',
  'sidebar-fg-muted',
  'sidebar-hover',
  'sidebar-active-bg',
  'sidebar-active-fg',
  'sidebar-border',
  'content-bg',
  'content-fg',
  'content-fg-muted',
  'content-hover',
  'content-border',
  'control-border',
  'surface-raised',
  'surface-sunken',
  'highlight-bg',
  'accent',
  'accent-hover',
  'on-accent',
  'accent-soft',
  'link',
  'focus-ring',
  'mention-fg',
  'mention-bg',
  'badge-bg',
  'badge-fg',
  'presence-online',
  'success',
  'warning',
  'danger',
  'agent',
  'agent-soft',
  'agent-working',
]

/** Slots that follow a base color until someone sets them directly. */
export const DERIVED_SLOTS: Record<string, string> = {
  'rail-tile': 'color-mix(in oklch, var(--rail-fg) 12%, var(--rail-bg))',
  'sidebar-hover': 'color-mix(in oklch, var(--sidebar-fg-strong) 9%, var(--sidebar-bg))',
  'sidebar-border': 'color-mix(in oklch, var(--sidebar-fg-strong) 12%, var(--sidebar-bg))',
  'sidebar-fg-muted': 'color-mix(in oklch, var(--sidebar-fg) 82%, var(--sidebar-bg))',
  'content-hover': 'color-mix(in oklch, var(--content-fg) 3%, var(--content-bg))',
  'content-border': 'color-mix(in oklch, var(--content-fg) 9%, var(--content-bg))',
  'surface-sunken': 'color-mix(in oklch, var(--content-fg) 4%, var(--content-bg))',
  'accent-hover': 'color-mix(in oklch, var(--accent) 86%, black)',
  'accent-soft': 'color-mix(in oklch, var(--accent) 10%, var(--content-bg))',
  'focus-ring': 'var(--accent)',
  'mention-fg': 'color-mix(in oklch, var(--accent) 88%, var(--content-fg))',
  'mention-bg': 'color-mix(in oklch, var(--accent) 12%, var(--content-bg))',
  'agent-soft': 'color-mix(in oklch, var(--agent) 12%, var(--content-bg))',
}

/** Custom properties for a set of overrides: the overrides, plus any derived slot whose base changed. */
export function themeProperties(overrides: Record<string, string>) {
  const properties: Record<string, string> = {}
  const changed = new Set(Object.keys(overrides))
  for (const [slot, formula] of Object.entries(DERIVED_SLOTS)) {
    if (overrides[slot]) continue
    const bases = (formula.match(/--[a-z-]+/g) ?? []).map((name) => name.slice(2))
    if (bases.some((base) => changed.has(base))) properties[`--${slot}`] = formula
  }
  for (const [slot, color] of Object.entries(overrides)) properties[`--${slot}`] = color
  return properties
}

export type Rgb = readonly [number, number, number]

export function toHex([r, g, b]: Rgb) {
  return `#${[r, g, b].map((n) => Math.round(n).toString(16).padStart(2, '0')).join('')}`
}

const HEX = /^#[0-9a-f]{6}$/i

export function sanitizeOverrides(value: unknown): Record<string, string> {
  const result: Record<string, string> = {}
  if (!value || typeof value !== 'object' || Array.isArray(value)) return result
  for (const [slot, color] of Object.entries(value as Record<string, unknown>)) {
    // Only known slots and plain hex colors ever reach CSS.
    if (ALL_SLOTS.includes(slot) && typeof color === 'string' && HEX.test(color)) {
      result[slot] = color.toLowerCase()
    }
  }
  return result
}
