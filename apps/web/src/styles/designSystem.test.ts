/// <reference types="node" />
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'

import { describe, expect, it } from 'vitest'

// Keeps every stylesheet on the design system in apps/web/DESIGN.md: colors,
// type, spacing, radii, focus rings and motion come from the tokens in
// tokens.css. A value that truly can't (an emoji's size, a viewer that is
// always dark) says why on the same line with `/* design-exception: … */`.

// Vitest runs from apps/web; import.meta.url isn't a file URL under jsdom.
const src = join(process.cwd(), 'src')

function stylesheets(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) return stylesheets(path)
    return name.endsWith('.css') && name !== 'tokens.css' ? [path] : []
  })
}

type Declaration = { file: string; line: number; property: string; value: string; exempt: boolean }

function declarations(path: string): Declaration[] {
  const file = relative(src, path)
  const lines = readFileSync(path, 'utf8').split('\n')
  let inComment = false
  return lines.flatMap((text, index) => {
    const exempt = text.includes('design-exception:')
    let code = text
    if (inComment) {
      const end = code.indexOf('*/')
      if (end === -1) return []
      code = code.slice(end + 2)
      inComment = false
    }
    code = code.replace(/\/\*.*?\*\//g, '')
    const open = code.indexOf('/*')
    if (open !== -1) {
      code = code.slice(0, open)
      inComment = true
    }
    const match = /^\s*([a-z-]+)\s*:\s*(.+?);?\s*$/.exec(code)
    if (!match || match[1].startsWith('--')) return []
    return [{ file, line: index + 1, property: match[1], value: match[2], exempt }]
  })
}

const all = stylesheets(src).flatMap(declarations)
const where = (d: Declaration) => `${d.file}:${d.line}  ${d.property}: ${d.value}`

function violations(test: (d: Declaration) => boolean) {
  return all.filter((d) => !d.exempt && test(d)).map(where)
}

const COLOR = /#[0-9a-f]{3,8}\b|\b(?:rgba?|hsla?|oklch|oklab|lab|lch)\(|\b(?:black|white|red|green|blue|gray|grey|orange|yellow|purple|pink)\b/i
const SPACING = /^(?:padding|margin|gap|row-gap|column-gap)(?:-[a-z-]+)?$/
// The type scale's sizes. Anything else is a new style: add it to tokens.css first.
const TYPE_SIZES = new Set([11, 12, 13, 14, 15, 16, 18, 24, 28])

describe('design system', () => {
  it('reads colors from theme slots, never literals', () => {
    expect(violations((d) => d.property !== 'white-space' && COLOR.test(d.value.replace(/white-space/g, '')))).toEqual([])
  })

  it('sets type with the --t-* styles and --weight-* tokens', () => {
    const offScale = (value: string) =>
      [...value.matchAll(/(\d+(?:\.\d+)?)px/g)].some(([, size], i) => i === 0 && !TYPE_SIZES.has(Number(size)))
    expect(
      violations(
        (d) =>
          (d.property === 'font-weight' && /^\d/.test(d.value)) ||
          (d.property === 'font' && /^\d{3}\s/.test(d.value)) ||
          (d.property === 'font-size' && /\dpx/.test(d.value) && (!d.value.includes('--text-scale') || offScale(d.value))),
      ),
    ).toEqual([])
  })

  it('spaces with the --space-* scale', () => {
    // 0 to 3px are hairline nudges; larger off-grid lengths (an icon column's width) are layout, not spacing.
    const onGrid = /(?<![\w.-])-?(?:4|5|6|7|8|9|10|12|16|20|24|32|40|48)px\b/
    expect(violations((d) => SPACING.test(d.property) && !d.value.includes('calc(') && onGrid.test(d.value))).toEqual([])
  })

  it('rounds corners with the --radius-* scale', () => {
    expect(violations((d) => /radius/.test(d.property) && /(?<![\w.])(?:[3-9]|\d{2,})px|%/.test(d.value.replace(/calc\([^)]*\)/g, '')))).toEqual(
      [],
    )
  })

  it('draws one focus ring', () => {
    // A box-shadow made from the focus color alone was invalid CSS, which left those controls with no ring.
    expect(
      violations(
        (d) => (d.property === 'box-shadow' && /^var\(--focus-ring\)$/.test(d.value)) || (d.property === 'outline' && /\dpx solid/.test(d.value)),
      ),
    ).toEqual([])
  })

  it('times transitions with the --duration-* tokens and layers with --layer-*', () => {
    expect(
      violations(
        (d) =>
          (/^transition(?:-duration)?$/.test(d.property) && /\d(?:ms|s)\b/.test(d.value.replace(/var\([^)]*\)/g, ''))) ||
          (d.property === 'z-index' && /^\d{2,}$/.test(d.value)),
      ),
    ).toEqual([])
  })
})
