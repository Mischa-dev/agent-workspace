import { describe, expect, it } from 'vitest'

import { isWorkspacePath, legacyPathOf, workspaceHref, workspaceSlugOf } from './href'

describe('workspace addresses', () => {
  it('builds a workspace page’s address from its old path or its new one', () => {
    expect(workspaceHref('acme')).toBe('/w/acme/home')
    expect(workspaceHref('acme', '/inbox?tab=all#top')).toBe('/w/acme/inbox?tab=all#top')
    expect(workspaceHref('acme', '/projects')).toBe('/w/acme/spaces')
    expect(workspaceHref('acme', '/projects/new')).toBe('/w/acme/spaces/new')
    expect(workspaceHref('acme', '/new-task?project=p1')).toBe('/w/acme/new?project=p1')
    expect(workspaceHref('acme', '/spaces')).toBe('/w/acme/spaces')
    expect(workspaceHref('acme', '/projectsx')).toBe('/w/acme/projectsx')
  })

  it('reads an address back in its old shape', () => {
    expect(legacyPathOf('/w/acme')).toBe('/home')
    expect(legacyPathOf('/w/acme/')).toBe('/home')
    expect(legacyPathOf('/w/acme/spaces/new')).toBe('/projects/new')
    expect(legacyPathOf('/w/acme/new')).toBe('/new-task')
    expect(legacyPathOf('/w/acme/dm/c1')).toBe('/messages/c1')
    expect(legacyPathOf('/w/acme/runs/r1')).toBe('/runs/r1')
    expect(legacyPathOf('/projects/p1/pages')).toBe('/projects/p1/pages')
    expect(workspaceSlugOf('/w/acme/inbox')).toBe('acme')
    expect(workspaceSlugOf('/inbox')).toBeNull()
  })

  it('knows which old addresses are a workspace’s own pages', () => {
    for (const path of ['/home', '/inbox', '/runs/:runId', '/projects', '/projects/new', '/messages/x', '/dm/x', '/search?q=a']) {
      expect(isWorkspacePath(path), path).toBe(true)
    }
    for (const path of ['/', '/projects/p1', '/projects/:projectId/pages', '/settings/account', '/AWD-12', '/:taskKey', '*', '/w/acme/home', '/sign-in', '/forms/:token']) {
      expect(isWorkspacePath(path), path).toBe(false)
    }
  })
})
