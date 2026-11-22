import { describe, expect, it } from 'vitest'

import type { ThemeDocument } from '../api/generated/operations'
import { applyThemeCss, documentCss } from './customTheme'

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

describe('applyThemeCss', () => {
  it('stays after a stylesheet a page loads later', async () => {
    applyThemeCss('.aw-prop { gap: 0; }')
    const later = document.createElement('link')
    later.rel = 'stylesheet'
    document.head.appendChild(later)
    await new Promise((done) => setTimeout(done, 0))
    expect(document.head.lastElementChild?.id).toBe('aw-theme-css')
    applyThemeCss('')
    later.remove()
  })
})
