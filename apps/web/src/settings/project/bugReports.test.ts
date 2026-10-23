import { describe, expect, it } from 'vitest'

import { SNIPPET_KINDS, snippet } from './bugReports'

const URL = 'https://workspace.example.com/api/v1/bug-reports/awbr_abc123'

describe('bug report snippets', () => {
  it('puts the link in every snippet', () => {
    for (const { value } of SNIPPET_KINDS) {
      expect(snippet(value, URL, 'Ragdoll Rapture', 'Games')).toContain(URL)
    }
  })

  it('tells an agent the app, the project and what to send', () => {
    const text = snippet('agent', URL, 'Ragdoll Rapture', 'Games')
    expect(text).toContain('Ragdoll Rapture')
    expect(text).toContain('the Games project')
    expect(text).toContain('"title"')
  })
})
