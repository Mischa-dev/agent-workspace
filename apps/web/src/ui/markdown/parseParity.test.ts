/// <reference types="node" />
import { readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import { parseMarkdown } from './parse'

// The server parses pages the same way (apps/server/src/page_docs/markdown.rs,
// AWD-53), so a page keeps its look when it becomes a live document. Both
// sides check every case here. After a change to parse.ts, rewrite the trees
// with UPDATE_PARSE_PARITY=1 and port the change to the server.

// Vitest runs from apps/web; import.meta.url isn't a file URL under jsdom.
const FILE = join(process.cwd(), 'src/ui/markdown/parseParity.json')

type Case = { name: string; markdown: string; blocks: unknown }
type Parity = { about: string; cases: Case[] }

const parity = JSON.parse(readFileSync(FILE, 'utf8')) as Parity

describe('Markdown trees shared with the server', () => {
  it('has its cases', () => {
    expect(parity.cases.length).toBeGreaterThanOrEqual(60)
    expect(new Set(parity.cases.map((testCase) => testCase.name)).size).toBe(parity.cases.length)
  })

  if (process.env.UPDATE_PARSE_PARITY) {
    it('rewrites the trees', () => {
      const cases = parity.cases.map((testCase) => ({ ...testCase, blocks: parseMarkdown(testCase.markdown) }))
      const lines = cases.map((testCase) => `    ${JSON.stringify(testCase)}`)
      writeFileSync(FILE, `{\n  "about": ${JSON.stringify(parity.about)},\n  "cases": [\n${lines.join(',\n')}\n  ]\n}\n`)
    })
    return
  }

  it.each(parity.cases.map((testCase) => [testCase.name, testCase] as const))('%s', (_name, testCase) => {
    // Through JSON, as the server compares it: undefined fields drop out.
    expect(JSON.parse(JSON.stringify(parseMarkdown(testCase.markdown)))).toEqual(testCase.blocks)
  })
})
