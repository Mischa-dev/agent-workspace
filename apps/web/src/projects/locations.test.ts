import { describe, expect, it } from 'vitest'

import {
  normalizeLocation,
  projectLocationFromPath,
  projectLocationHref,
  safeProjectHref,
} from './locations'

describe('structured Project locations', () => {
  it('round-trips Project overview, settings, Work Items, Channel, and Thread locations', () => {
    const projectId = 'project-a'
    const thread = { kind: 'thread', conversationId: 'thread-a' } as const
    expect(projectLocationHref(projectId, { kind: 'project_overview' })).toBe(
      '/projects/project-a',
    )
    // A Project that last remembered its settings opens on its overview.
    expect(projectLocationHref(projectId, { kind: 'settings' })).toBe(
      '/projects/project-a',
    )
    expect(projectLocationHref(projectId, { kind: 'work_items' })).toBe(
      '/projects/project-a/work-items',
    )
    expect(
      projectLocationFromPath(projectId, '/projects/project-a/work-items/item-a'),
    ).toEqual({ kind: 'work_items' })
    const href = projectLocationHref(projectId, thread)
    expect(projectLocationFromPath(projectId, href)).toEqual(thread)
    const channel = { kind: 'channel', conversationId: 'channel-a' } as const
    const channelHref = projectLocationHref(projectId, channel)
    expect(channelHref).toBe('/projects/project-a/channels/channel-a')
    expect(projectLocationFromPath(projectId, channelHref)).toEqual(channel)
  })

  it('remembers each task tab, the pages, one page, the decisions and the runs', () => {
    const projectId = 'project-a'
    const page = { kind: 'page', pageId: 'page-a' } as const
    expect(projectLocationHref(projectId, page)).toBe('/projects/project-a/pages/page-a')
    expect(projectLocationFromPath(projectId, '/projects/project-a/pages/page-a')).toEqual(page)
    expect(projectLocationFromPath(projectId, '/projects/project-a/pages')).toEqual({ kind: 'pages' })
    // Every planning page is the Planning tab, which opens on Up next.
    expect(projectLocationFromPath(projectId, '/projects/project-a/graph')).toEqual({ kind: 'planning' })
    expect(projectLocationHref(projectId, { kind: 'planning' })).toBe('/projects/project-a/next')
    expect(projectLocationFromPath(projectId, '/projects/project-a/launches')).toEqual({ kind: 'runs' })
    expect(projectLocationHref(projectId, { kind: 'runs' })).toBe('/projects/project-a/launches')
    for (const kind of ['milestones', 'timeline', 'decisions'] as const) {
      expect(projectLocationFromPath(projectId, projectLocationHref(projectId, { kind }))).toEqual({ kind })
    }
    expect(normalizeLocation({ kind: 'page', pageId: 'page-a' })).toEqual(page)
    // A page without its id isn't a place; the overview is.
    expect(normalizeLocation({ kind: 'page' })).toEqual({ kind: 'project_overview' })
    expect(normalizeLocation({ kind: 'toString' })).toEqual({ kind: 'project_overview' })
  })

  it('rejects arbitrary and cross-Project hrefs', () => {
    const location = { kind: 'thread', conversationId: 'thread-a' } as const
    expect(
      safeProjectHref('project-a', 'https://attacker.invalid', location),
    ).toBe('/projects/project-a/threads/thread-a')
    expect(
      projectLocationFromPath(
        'project-a',
        '/projects/project-b/threads/thread-a',
      ),
    ).toBeNull()
    expect(normalizeLocation({ kind: 'external', href: '//attacker' })).toEqual({
      kind: 'project_overview',
    })
  })
})
