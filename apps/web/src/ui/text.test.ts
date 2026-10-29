import { describe, expect, it } from 'vitest'

import { countLabel, plainPreview, withEllipses } from './text'

describe('countLabel', () => {
  it('counts with the right noun', () => {
    expect(countLabel(1, 'task')).toBe('1 task')
    expect(countLabel(0, 'task')).toBe('0 tasks')
    expect(countLabel(3, 'reply', 'replies')).toBe('3 replies')
  })
})

describe('plainPreview', () => {
  it('drops Markdown syntax and keeps the words', () => {
    expect(plainPreview('Here are the **onboarding** copy drafts:\n1. Name your workspace\n2. Create a project')).toBe(
      'Here are the onboarding copy drafts: Name your workspace Create a project',
    )
    expect(plainPreview('# Release checklist\n- [ ] Run the full CI\n- [x] Back up')).toBe('Release checklist Run the full CI Back up')
    expect(plainPreview('See [the board](https://example.com/a_(b)) and `pnpm dev`')).toBe('See the board and pnpm dev')
    expect(plainPreview('> quoted _text_ with snake_case_name and 2 * 3 * 4')).toBe('quoted text with snake_case_name and 2 * 3 * 4')
    expect(plainPreview('```ts\nexport function greet() {}\n```\nafter')).toBe('export function greet() {} after')
    expect(plainPreview('| a | b |\n|---|---|\n| 1 | 2 |')).toBe('a b 1 2')
  })

  it('cuts long text on a word boundary', () => {
    expect(plainPreview('one two three four five six', 14)).toBe('one two three…')
    expect(plainPreview('', 10)).toBe('')
    expect(plainPreview(null)).toBe('')
  })
})

describe('withEllipses', () => {
  it('marks where a fragment was cut mid-sentence', () => {
    expect(withEllipses('the theme follows the device, which is why it')).toBe('…the theme follows the device, which is why it…')
    expect(withEllipses('The theme follows the device.')).toBe('The theme follows the device.')
    expect(withEllipses('\u0001Theme\u0002 follows the device')).toBe('\u0001Theme\u0002 follows the device…')
    expect(withEllipses('Cut already…')).toBe('Cut already…')
    expect(withEllipses('')).toBe('')
  })
})
