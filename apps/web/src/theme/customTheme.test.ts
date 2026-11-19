import { describe, expect, it } from 'vitest'

import { documentCss, type ThemeDocument } from './customTheme'

const withCss = (css: string) =>
  ({ format: 1, light: { base: 'quiet', tokens: {} }, dark: { base: 'quiet-dark', tokens: {} }, css }) as unknown as ThemeDocument

describe('documentCss', () => {
  it('keeps a stylesheet whose comment names a blocked function', () => {
    const css = '/* url(x) is not allowed */ .aw-card { border: 0; }'
    expect(documentCss(withCss(css))).toBe(css)
  })

  it('drops a stylesheet that loads from elsewhere', () => {
    expect(documentCss(withCss('.aw-card { background: url(https://example.com/a); }'))).toBe('')
    expect(documentCss(withCss('/* note */ @import "x.css";'))).toBe('')
  })
})
