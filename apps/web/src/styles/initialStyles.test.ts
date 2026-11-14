import { describe, expect, it } from 'vitest'

import mainSource from '../main.tsx?raw'

// W1.15: the first paint loads only the design system and the shell. A
// feature's stylesheet is imported by the components that use it, so it
// arrives with their route.
describe('the first paint', () => {
  it('loads only the shared stylesheets', () => {
    const sheets = [...mainSource.matchAll(/^import '\.\/styles\/([^']+)'$/gm)].map((match) => match[1])
    expect(sheets).toEqual(['tokens.css', 'base.css', 'components.css', 'shell.css', 'kit.css'])
    const others = [...mainSource.matchAll(/^import '([^']+\.css)'$/gm)].map((match) => match[1])
    expect(others.filter((sheet) => !sheet.startsWith('./styles/') && !sheet.startsWith('@fontsource'))).toEqual([])
  })
})
