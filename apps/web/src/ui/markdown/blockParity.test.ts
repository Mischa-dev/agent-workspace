import { describe, expect, it } from 'vitest'

import parity from './blockParity.json'
import { parseMarkdown, type Block } from './parse'

// The server reads pages as the top-level blocks this parser draws
// (apps/server/src/page_blocks.rs, AWD-53). Both sides check the same cases.

type Case = { name: string; markdown?: string; generate?: { repeat: string; count: number; then: string }; blocks: unknown[] }

function markdownOf(testCase: Case) {
  if (testCase.markdown !== undefined) return testCase.markdown
  if (!testCase.generate) throw new Error(`${testCase.name}: no markdown`)
  const { repeat, count, then } = testCase.generate
  return repeat.repeat(count) + then
}

function summary(block: Block) {
  switch (block.type) {
    case 'heading':
      return { type: block.type, level: block.level }
    case 'code':
      return { type: block.type, lang: block.lang }
    case 'list':
      return {
        type: block.type,
        ordered: block.ordered,
        start: block.start,
        items: block.items.length,
        tasks: block.items.filter((item) => item.checked !== null).length,
      }
    case 'table':
      return { type: block.type, columns: block.head.length, rows: block.rows.length }
    default:
      return { type: block.type }
  }
}

describe('top-level blocks shared with the server', () => {
  const cases = parity.cases as Case[]

  it('has its cases', () => {
    expect(cases.length).toBeGreaterThanOrEqual(20)
  })

  it.each(cases.map((testCase) => [testCase.name, testCase] as const))('%s', (_name, testCase) => {
    expect(parseMarkdown(markdownOf(testCase)).map(summary)).toEqual(testCase.blocks)
  })
})
