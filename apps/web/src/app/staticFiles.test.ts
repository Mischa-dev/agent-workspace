import { describe, expect, it } from 'vitest'

import page from '../../index.html?raw'

// W1.38 (R24.1): the web app is static files any host can serve. Nothing in
// index.html is filled in by a server; the app asks /client-config at start.
describe('the web app as static files', () => {
  it('has no server-written value in index.html', () => {
    for (const marker of ['{{', '{%', '<%', '<!--#', '%VITE_', '__AW_', '${']) {
      expect(page, `template marker ${marker}`).not.toContain(marker)
    }
    // No origin baked in: every reference is to this site's own root.
    expect(page).not.toMatch(/(?:src|href)="https?:/)
  })
})
