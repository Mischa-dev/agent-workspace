import { describe, expect, it } from 'vitest'

import { projectSection } from './projectSections'

describe('projectSection', () => {
  const base = '/projects/p1'

  it('puts milestones, timeline and planning under Work items, and decisions under Pages', () => {
    expect(projectSection(`${base}/milestones`, base)).toBe('work')
    expect(projectSection(`${base}/graph`, base)).toBe('work')
    expect(projectSection(`${base}/work-items/t1`, base)).toBe('work')
    expect(projectSection(`${base}/decisions`, base)).toBe('pages')
    expect(projectSection(`${base}/pages/x`, base)).toBe('pages')
  })

  it('leaves the overview, channels and other Projects alone', () => {
    expect(projectSection(base, base)).toBeNull()
    expect(projectSection(`${base}/channels/c1`, base)).toBeNull()
    expect(projectSection('/projects/p10/milestones', base)).toBeNull()
  })
})
