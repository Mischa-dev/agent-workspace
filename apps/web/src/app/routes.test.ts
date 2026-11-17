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
    // A project's own pages, settings and task keys stay where they are until W1.14b.
    expect(handle('/projects/:projectId/work-items')?.layout).toBe('shell')
    expect(handle('/settings/*')?.layout).toBe('shell')
    expect(handle('/:taskKey')?.layout).toBe('shell')
  })
})
