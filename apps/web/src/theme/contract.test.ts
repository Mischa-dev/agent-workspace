/// <reference types="node" />
import { readFileSync, readdirSync } from 'node:fs'

import { describe, expect, it } from 'vitest'

import contract from './contract.json'
import { PARTS, THEME_TOKENS, blankDocument, cssProblem, documentProblems, readDocument, valueProblem } from './contract'
import { THEME_PRESETS } from './slots'

// Read from disk: vitest hands CSS imports over empty.
const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8')
const cssFiles = (readdirSync('src', { recursive: true }) as string[]).filter((name) => name.endsWith('.css')).sort()
const css = Object.fromEntries(cssFiles.map((name) => [name, readFileSync(`src/${name}`, 'utf8')]))
const styles = Object.values(css).join('\n')

/**
 * Shared pieces' blocks a theme doesn't style by name: parts of a published
 * piece, or pieces still to be published. A new block in src/ui or
 * src/pieces is either a part in contract.json or listed here with a reason.
 */
const UNPUBLISHED_BLOCKS: Record<string, string> = {
  'aw-ccal': "the calendar piece's month grid, inside .aw-calendar",
  'aw-combobox-trigger': "the combobox's button, inside .aw-combobox",
  'aw-pboard-wrap': "the board piece's scroller, inside .aw-pboard",
  'aw-piece-sample': 'the design system page only',
  'aw-records': "collections' record list, inside .aw-piece-table",
}

const RAW_COLOR = /#[0-9a-f]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch)\(\s*[\d.]/i

describe('the theme contract', () => {
  it('lists every built-in theme, as the app has them', () => {
    expect(contract.presets).toEqual(THEME_PRESETS.map((preset) => ({ id: preset.id, name: preset.name, scheme: preset.scheme })))
  })

  it('names only tokens the stylesheets define, and part classes they style', () => {
    const tokensCss = read('../styles/tokens.css')
    for (const token of THEME_TOKENS) expect(tokensCss, token).toContain(`--${token}:`)
    for (const part of PARTS) expect(styles, part.class).toContain(`.${part.class}`)
  })

  it('styles the app through tokens: a colour written out in CSS says why on its line', () => {
    const loose = Object.entries(css)
      .filter(([name]) => name !== 'styles/tokens.css')
      .flatMap(([name, text]) =>
        text
          .split('\n')
          .map((line, index) => ({ line, at: `${name}:${index + 1}` }))
          .filter(({ line }) => RAW_COLOR.test(line.replace(/\/\*.*?\*\//g, '')) && !line.includes('design-exception:'))
          .map(({ at, line }) => `${at}: ${line.trim()} (use a token, or add /* design-exception: why */)`),
      )
    expect(loose).toEqual([])
  })

  it('publishes a part name for every shared piece', () => {
    const published = new Set(PARTS.map((part) => part.class))
    const blocks = new Set<string>()
    for (const [name, text] of Object.entries(css)) {
      if (!name.startsWith('ui/') && !name.startsWith('pieces/')) continue
      for (const [, block] of text.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/\.(aw-[a-z0-9-]+?)(?:__|--|[^a-z0-9-]|$)/g)) blocks.add(block)
    }
    const unnamed = [...blocks].filter((block) => !published.has(block) && !(block in UNPUBLISHED_BLOCKS))
    expect(unnamed, 'add each to contract.json parts (or UNPUBLISHED_BLOCKS here, with why)').toEqual([])
  })

  it('takes colours, lengths and math, and refuses anything that loads or escapes', () => {
    for (const value of ['#224466', 'oklch(0.5 0.1 200)', 'color-mix(in oklch, var(--accent) 20%, white)', 'calc(var(--space-2) * 2)', '"Inter", sans-serif'])
      expect(valueProblem(value), value).toBeNull()
    for (const value of ['url(https://x.invalid/a.png)', 'red; background: red', 'image-set("a.png" 1x)', 'var(--not-a-token)', '\\75rl(x)', '(', '"open'])
      expect(valueProblem(value), value).not.toBeNull()
  })

  it('takes CSS on part names, and refuses CSS that loads anything or breaks out', () => {
    expect(cssProblem('.aw-card { border-radius: var(--radius-lg); }\n@media (max-width: 600px) { .aw-sidebar { display: none; } }')).toBeNull()
    for (const css of [
      '.aw-card { background: url(https://x.invalid/a.png); }',
      '.aw-card { background: URL(x); }',
      '@import "https://x.invalid/a.css";',
      '@font-face { font-family: x; src: local(x); }',
      '.aw-card { background: \\75 rl(x); }',
      '</style><script>alert(1)</script>',
      '.aw-card { color: red;',
      '/* never closed',
      'input[value^="a"] { background-image: image-set("x" 1x); }',
    ])
      expect(cssProblem(css), css).not.toBeNull()
  })

  it('reads a theme file back only when it passes', () => {
    const document = { ...blankDocument('quiet', 'quiet-dark'), light: { base: 'quiet', tokens: { accent: '#224466' } } }
    expect(documentProblems(document)).toEqual({})
    expect(readDocument({ name: 'Harbour', ...document })).toEqual(document)
    expect(readDocument({ ...document, light: { base: 'quiet-dark', tokens: {} } })).toBeNull()
    expect(readDocument({ ...document, css: '@import "x.css";' })).toBeNull()
    expect(readDocument('not a theme')).toBeNull()
  })
})
