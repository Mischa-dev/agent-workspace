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
    expect(handle('/whiteboards/:whiteboardId')).toEqual({ space: 'home', layout: 'canvas' })
    expect(handle('/projects/:projectId/pages/:pageId')?.space).toBe('project')
    expect(handle('/messages/:conversationId')?.space).toBe('direct')
  })
})
