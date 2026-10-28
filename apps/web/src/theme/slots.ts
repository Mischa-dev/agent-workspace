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

/** [slot, label, shown in the main list] */
export type SlotEntry = readonly [slot: string, label: string, main?: boolean]

export const SLOT_GROUPS: readonly { label: string; slots: readonly SlotEntry[] }[] = [
  {
    label: 'Rail',
    slots: [
      ['rail-bg', 'Background', true],
      ['rail-fg', 'Icons'],
      ['rail-tile', 'Tiles'],
      ['rail-tile-active', 'Selected tile', true],
      ['rail-indicator', 'Indicator'],
    ],
  },
  {
    label: 'Sidebar',
    slots: [
      ['sidebar-bg', 'Background', true],
      ['sidebar-fg', 'Text', true],
      ['sidebar-fg-strong', 'Unread text'],
      ['sidebar-fg-muted', 'Labels and icons'],
      ['sidebar-hover', 'Hover'],
      ['sidebar-active-bg', 'Selected row', true],
      ['sidebar-active-fg', 'Selected text', true],
      ['sidebar-border', 'Divider'],
    ],
  },
  {
    label: 'Messages',
    slots: [
      ['content-bg', 'Background', true],
      ['content-fg', 'Text', true],
      ['content-fg-muted', 'Secondary text'],
      ['content-hover', 'Hover'],
      ['content-border', 'Dividers'],
      ['control-border', 'Input borders'],
      ['surface-raised', 'Menus and composer'],
      ['surface-sunken', 'Code and quotes'],
      ['highlight-bg', 'Mentions of you', true],
    ],
  },
  {
    label: 'Accent and links',
    slots: [
      ['accent', 'Accent', true],
      ['accent-hover', 'Accent hover'],
      ['on-accent', 'Text on accent'],
      ['accent-soft', 'Soft accent'],
      ['link', 'Links', true],
      ['focus-ring', 'Focus ring'],
      ['mention-fg', 'Mention text'],
      ['mention-bg', 'Mention fill'],
    ],
  },
  {
    label: 'Badges and status',
    slots: [
      ['badge-bg', 'Mention badge', true],
      ['badge-fg', 'Badge text'],
      ['presence-online', 'Online', true],
      ['success', 'Success'],
      ['warning', 'Warning'],
      ['danger', 'Danger'],
    ],
  },
  {
    label: 'Agents',
    slots: [
      ['agent', 'Agent tag', true],
      ['agent-soft', 'Agent tag fill'],
      ['agent-working', 'Working', true],
    ],
  },
]

export const ALL_SLOTS: readonly string[] = SLOT_GROUPS.flatMap((group) => group.slots.map((entry) => entry[0]))

export const SLOT_LABELS: Record<string, string> = Object.fromEntries(
  SLOT_GROUPS.flatMap((group) => group.slots.map(([slot, label]) => [slot, `${group.label} ${label.toLocaleLowerCase()}`])),
)

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

/** Pairs that carry meaning: [text or mark, background, minimum ratio]. */
export const CONTRAST_PAIRS: readonly [string, string, number][] = [
  ['sidebar-fg', 'sidebar-bg', 4.5],
  ['sidebar-fg-strong', 'sidebar-bg', 4.5],
  ['sidebar-fg-muted', 'sidebar-bg', 4.5],
  ['sidebar-fg', 'sidebar-hover', 4.5],
  ['sidebar-active-fg', 'sidebar-active-bg', 4.5],
  ['rail-fg', 'rail-bg', 3],
  ['on-accent', 'rail-tile-active', 3],
  ['content-fg', 'content-bg', 4.5],
  ['content-fg-muted', 'content-bg', 4.5],
  ['content-fg', 'highlight-bg', 4.5],
  ['content-fg-muted', 'surface-sunken', 4.5],
  ['link', 'content-bg', 4.5],
  ['mention-fg', 'mention-bg', 4.5],
  ['on-accent', 'accent', 4.5],
  ['accent', 'content-bg', 3],
  ['control-border', 'content-bg', 3],
  ['focus-ring', 'content-bg', 3],
  ['badge-fg', 'badge-bg', 4.5],
  ['presence-online', 'sidebar-bg', 3],
  ['presence-online', 'content-bg', 3],
  ['agent', 'agent-soft', 4.5],
  ['agent-working', 'sidebar-bg', 3],
  ['agent-working', 'content-bg', 3],
  ['success', 'content-bg', 4.5],
  ['warning', 'content-bg', 4.5],
  ['danger', 'content-bg', 4.5],
]

export type Rgb = readonly [number, number, number]

export function relativeLuminance([r, g, b]: Rgb) {
  const channel = (value: number) => {
    const c = value / 255
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

export function contrastRatio(a: Rgb, b: Rgb) {
  const [high, low] = [relativeLuminance(a), relativeLuminance(b)].sort((x, y) => y - x)
  return (high + 0.05) / (low + 0.05)
}

export function toHex([r, g, b]: Rgb) {
  return `#${[r, g, b].map((n) => Math.round(n).toString(16).padStart(2, '0')).join('')}`
}

/** Walk a text color toward black or white until it reads on the background. */
export function fixContrast(foreground: Rgb, background: Rgb, floor: number): string {
  const target = relativeLuminance(background) > 0.18 ? [0, 0, 0] : [255, 255, 255]
  let color: Rgb = foreground
  for (let step = 0.05; step <= 1.001 && contrastRatio(color, background) < floor + 0.05; step += 0.05) {
    color = foreground.map((value, index) => value + (target[index] - value) * step) as unknown as Rgb
  }
  return toHex(color)
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

function base64UrlEncode(text: string) {
  return btoa(String.fromCharCode(...new TextEncoder().encode(text)))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '')
}

function base64UrlDecode(text: string) {
  const binary = atob(text.replace(/-/g, '+').replace(/_/g, '/'))
  return new TextDecoder().decode(Uint8Array.from(binary, (character) => character.charCodeAt(0)))
}

export function encodeTheme(preset: ThemePreset, overrides: Record<string, string>) {
  return `AW1.${base64UrlEncode(JSON.stringify({ b: preset, o: sanitizeOverrides(overrides) }))}`
}

export class ThemeCodeError extends Error {}

export function decodeTheme(code: string): { preset: ThemePreset; overrides: Record<string, string> } {
  const match = /^AW1\.([A-Za-z0-9_-]+)$/.exec(code.trim())
  if (!match) throw new ThemeCodeError('That isn’t a theme code. Theme codes start with AW1.')
  let data: { b?: unknown; o?: unknown }
  try {
    data = JSON.parse(base64UrlDecode(match[1])) as { b?: unknown; o?: unknown }
  } catch {
    throw new ThemeCodeError('That theme code is damaged. Copy it again and paste the whole thing.')
  }
  if (!isThemePreset(data.b)) throw new ThemeCodeError('That theme code is for a preset this version doesn’t have.')
  return { preset: data.b, overrides: sanitizeOverrides(data.o) }
}
