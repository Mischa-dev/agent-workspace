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
