import { describe, expect, it } from 'vitest'

import type { RunStep } from './api'
import { formatCost, formatTokens, logText, splitPatch } from './runModel'

describe('formatCost', () => {
  it('says nothing when the cost is unknown, but shows a known zero', () => {
    expect(formatCost(null, 'USD')).toBeNull()
    expect(formatCost(undefined, null)).toBeNull()
    expect(formatCost(0, 'USD')).toMatch(/0\.00/)
  })

  it('keeps enough digits for small amounts and survives a bad currency', () => {
    expect(formatCost(0.0042, 'USD')).toMatch(/0\.0042/)
    expect(formatCost(1.5, 'not-a-code')).toMatch(/1\.50/)
  })
})

describe('formatTokens', () => {
  it('shortens large counts', () => {
    expect(formatTokens(950)).toBe('950')
    expect(formatTokens(12_340)).toBe('12.3k')
    expect(formatTokens(4_100_000)).toBe('4.1M')
    expect(formatTokens(Number.NaN)).toBe('0')
  })
})

describe('splitPatch', () => {
  it('names each file by its new path, and a deletion by its old one', () => {
    const patch = [
      'diff --git a/src/a.ts b/src/a.ts',
      '--- a/src/a.ts',
      '+++ b/src/a.ts',
      '@@ -1 +1 @@',
      '-old',
      '+new',
      'diff --git a/gone.txt b/gone.txt',
      'deleted file mode 100644',
      '--- a/gone.txt',
      '+++ /dev/null',
      '@@ -1 +0,0 @@',
      '-bye',
      '',
    ].join('\n')
    const files = splitPatch(patch)
    expect(files.map((file) => file.path)).toEqual(['src/a.ts', 'gone.txt'])
    expect(files[0].text).toContain('+new')
    expect(files[0].text).not.toContain('bye')
  })

  it('returns nothing for an empty patch', () => {
    expect(splitPatch('')).toEqual([])
  })
})

describe('logText', () => {
  it('includes commands, exit codes and output', () => {
    const steps: RunStep[] = [
      {
        id: '1',
        kind: 'command_completed',
        summary: 'Ran tests',
        payload: { command: 'npm test', exitCode: 1, output: 'boom\n', truncated: true },
        createdAt: '2026-10-17T10:00:00Z',
      },
    ]
    const text = logText(steps)
    expect(text).toContain('$ npm test')
    expect(text).toContain('exit 1')
    expect(text).toContain('boom')
    expect(text).toContain('(output cut short)')
  })
})
