import { describe, expect, it } from 'vitest'

import { projectSection } from './projectSections'

describe('projectSection', () => {
  const base = '/w/acme/s/p1'

  it('puts milestones, timeline and planning under Work items, and decisions under Pages', () => {
    expect(projectSection(`${base}/milestones`, base)).toBe('work')
    expect(projectSection(`${base}/planning/graph`, base)).toBe('work')
    expect(projectSection(`${base}/tasks/t1`, base)).toBe('work')
    expect(projectSection(`${base}/decisions`, base)).toBe('pages')
    expect(projectSection(`${base}/pages/x`, base)).toBe('pages')
  })

  it('leaves the overview, channels and other Projects alone', () => {
    expect(projectSection(base, base)).toBeNull()
    expect(projectSection(`${base}/chat/c1`, base)).toBeNull()
    expect(projectSection('/w/acme/s/p10/milestones', base)).toBeNull()
  })
})
