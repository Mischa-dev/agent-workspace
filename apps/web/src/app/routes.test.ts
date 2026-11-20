import { describe, expect, it } from 'vitest'

import { routePaths, routesWithHandles } from './routes'

describe('the route table', () => {
  it('gives every route a handle with its space and layout', () => {
    const missing = routesWithHandles()
      .filter((route) => !route.handle?.space || !route.handle.layout)
      .map((route) => route.path)
    expect(missing).toEqual([])
  })

  it('keeps each address once', () => {
    const paths = routePaths()
    expect(paths.length).toBeGreaterThan(30)
    expect(paths.filter((path, index) => paths.indexOf(path) !== index)).toEqual([])
  })

  it('frames the pages as before', () => {
    const handle = (path: string) => routesWithHandles().find((route) => route.path === path)?.handle
    expect(handle('/sign-in')).toEqual({ space: 'none', layout: 'public' })
    expect(handle('/w/:ws/whiteboards/:whiteboardId')).toEqual({ space: 'home', layout: 'canvas' })
    expect(handle('/projects/:projectId/pages/:pageId')?.space).toBe('project')
    expect(handle('/w/:ws/messages/:conversationId')?.space).toBe('direct')
    expect(handle('/w/:ws/dm/:conversationId')).toEqual({ space: 'direct', layout: 'shell' })
  })

  it('serves every workspace page under /w/:ws and redirects its old address (W1.14a)', () => {
    const paths = routePaths()
    const handle = (path: string) => routesWithHandles().find((route) => route.path === path)?.handle
    for (const [old, now] of [
      ['/home', '/w/:ws/home'],
      ['/inbox', '/w/:ws/inbox'],
      ['/runs/:runId', '/w/:ws/runs/:runId'],
      ['/projects', '/w/:ws/spaces'],
      ['/projects/new', '/w/:ws/spaces/new'],
      ['/new-task', '/w/:ws/new'],
      ['/dm/:conversationId', '/w/:ws/dm/:conversationId'],
      ['/whiteboards/:whiteboardId', '/w/:ws/whiteboards/:whiteboardId'],
    ]) {
      expect(paths).toContain(now)
      expect(handle(old)?.layout).toBe('redirect')
    }
    expect(handle('/')?.layout).toBe('redirect')
    // Account settings and task keys stay at the root.
    expect(handle('/settings/*')?.layout).toBe('shell')
    expect(handle('/:taskKey')?.layout).toBe('shell')
  })

  it('serves a space’s pages under /w/:ws/s/:projectId in the address’s words (W1.14b)', () => {
    const paths = routePaths()
    const handle = (path: string) => routesWithHandles().find((route) => route.path === path)?.handle
    for (const [old, now] of [
      ['/projects/:projectId', '/w/:ws/s/:projectId'],
      ['/projects/:projectId/work-items', '/w/:ws/s/:projectId/tasks'],
      ['/projects/:projectId/work-items/:workItemId', '/w/:ws/s/:projectId/tasks/:workItemId'],
      ['/projects/:projectId/channels/:channelId', '/w/:ws/s/:projectId/chat/:channelId'],
      ['/projects/:projectId/conversations/archived', '/w/:ws/s/:projectId/chat/archived'],
      ['/projects/:projectId/threads/:threadId', '/w/:ws/s/:projectId/threads/:threadId'],
      ['/projects/:projectId/launches', '/w/:ws/s/:projectId/runs'],
      ['/projects/:projectId/next', '/w/:ws/s/:projectId/planning/next'],
      ['/projects/:projectId/plan-import', '/w/:ws/s/:projectId/planning/import'],
    ]) {
      expect(paths).toContain(now)
      expect(handle(now)?.layout).not.toBe('redirect')
      expect(handle(old)?.layout).toBe('redirect')
    }
    // Any other old space address, and old settings links, redirect too.
    expect(handle('/projects/:projectId/*')?.layout).toBe('redirect')
    expect(handle('/settings/workspace/*')?.layout).toBe('redirect')
    expect(handle('/settings/projects/*')?.layout).toBe('redirect')
    expect(handle('/w/:ws/settings/*')?.layout).toBe('shell')
    expect(handle('/w/:ws/t/:kind/:id')?.layout).toBe('shell')
  })
})
