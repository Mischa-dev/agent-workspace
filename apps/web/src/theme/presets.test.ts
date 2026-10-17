/// <reference types="node" />
import { readFileSync } from 'node:fs'

import { describe, expect, it } from 'vitest'

import bootSource from '../../public/theme-boot.js?raw'
import { CONTRAST_PAIRS, THEME_PRESETS, contrastRatio, type Rgb } from './slots'

// Read from disk: vitest hands CSS imports over empty, even with ?raw.
const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8')
const tokensCss = read('../styles/tokens.css')
const classicCss = read('../../public/themes/classic.css')

/** Custom properties declared for each [data-theme="…"] selector. */
function themeBlocks(css: string) {
  const blocks = new Map<string, Record<string, string>>()
  for (const [, selector, body] of css.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const declarations = Object.fromEntries([...body.matchAll(/--([a-z0-9-]+):\s*([^;]+);/g)].map(([, name, value]) => [name, value.trim()]))
    for (const [, id] of selector.matchAll(/\[data-theme="([a-z0-9-]+)"\]/g)) blocks.set(id, { ...blocks.get(id), ...declarations })
  }
  return blocks
}

function oklchToRgb(l: number, c: number, h: number): Rgb {
  const a = c * Math.cos((h * Math.PI) / 180)
  const b = c * Math.sin((h * Math.PI) / 180)
  const lms = [l + 0.3963377774 * a + 0.2158037573 * b, l - 0.1055613458 * a - 0.0638541728 * b, l - 0.0894841775 * a - 1.291485548 * b].map(
    (value) => value ** 3,
  )
  const linear = [
    4.0767416621 * lms[0] - 3.3077115913 * lms[1] + 0.2309699292 * lms[2],
    -1.2684380046 * lms[0] + 2.6097574011 * lms[1] - 0.3413193965 * lms[2],
    -0.0041960863 * lms[0] - 0.7034186147 * lms[1] + 1.707614701 * lms[2],
  ]
  return linear.map((value) => {
    const clamped = Math.min(1, Math.max(0, value))
    return 255 * (clamped <= 0.0031308 ? 12.92 * clamped : 1.055 * clamped ** (1 / 2.4) - 0.055)
  }) as unknown as Rgb
}

function parseColor(value: string): Rgb {
  const hex = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(value)
  if (hex) return [parseInt(hex[1], 16), parseInt(hex[2], 16), parseInt(hex[3], 16)]
  const oklch = /^oklch\(([\d.]+) ([\d.]+) ([\d.]+)\)$/.exec(value)
  if (oklch) return oklchToRgb(Number(oklch[1]), Number(oklch[2]), Number(oklch[3]))
  throw new Error(`Can't read color ${value}`)
}

const blocks = new Map([...themeBlocks(tokensCss), ...themeBlocks(classicCss)])
const base = blocks.get('indigo') ?? {}

describe('theme presets', () => {
  it('defines every preset in a stylesheet and in the boot script', () => {
    for (const preset of THEME_PRESETS) {
      expect(blocks.has(preset.id), preset.id).toBe(true)
      expect(bootSource).toContain(`'${preset.id}'`)
      // Classic presets stand alone: none borrows a meaningful color from Indigo underneath.
      if (preset.group !== 'workspace') {
        for (const [fg, bg] of CONTRAST_PAIRS) expect(Object.keys(blocks.get(preset.id) ?? {}), preset.id).toEqual(expect.arrayContaining([fg, bg]))
      }
    }
  })

  it.each(THEME_PRESETS.map((preset) => [preset.name, preset.id]))('%s reads clearly on every pair the editor checks', (_name, id) => {
    const slots = { ...base, ...blocks.get(id) }
    const failing = CONTRAST_PAIRS.flatMap(([fg, bg, floor]) => {
      const ratio = contrastRatio(parseColor(slots[fg]), parseColor(slots[bg]))
      return ratio < floor ? [`${fg} on ${bg}: ${ratio.toFixed(2)} < ${floor}`] : []
    })
    expect(failing).toEqual([])
  })
})
