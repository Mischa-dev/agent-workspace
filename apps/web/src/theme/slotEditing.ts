/**
 * The Appearance page's side of the color slots: each slot's group and
 * label, and the contrast checks. It loads with that page and the
 * design system page; slots.ts holds what the app shell needs to apply a
 * saved theme.
 */
import { toHex, type Rgb } from './slots'

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

export const SLOT_LABELS: Record<string, string> = Object.fromEntries(
  SLOT_GROUPS.flatMap((group) => group.slots.map(([slot, label]) => [slot, `${group.label} ${label.toLocaleLowerCase()}`])),
)

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

/** Walk a text color toward black or white until it reads on the background. */
export function fixContrast(foreground: Rgb, background: Rgb, floor: number): string {
  const target = relativeLuminance(background) > 0.18 ? [0, 0, 0] : [255, 255, 255]
  let color: Rgb = foreground
  for (let step = 0.05; step <= 1.001 && contrastRatio(color, background) < floor + 0.05; step += 0.05) {
    color = foreground.map((value, index) => value + (target[index] - value) * step) as unknown as Rgb
  }
  return toHex(color)
}
