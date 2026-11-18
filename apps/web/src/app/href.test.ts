import { describe, expect, it } from 'vitest'

import {
  hrefFor,
  isWorkspacePath,
  legacyPathOf,
  setCurrentWorkspaceSlug,
  spaceHref,
  spaceIdOf,
  spaceSettingsHref,
  thingHref,
  workspaceHref,
  workspaceRoutePath,
  workspaceSlugOf,
} from './href'

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
    expect(legacyPathOf('/w/acme/s/p1')).toBe('/projects/p1')
    expect(legacyPathOf('/w/acme/s/p1/tasks/t1')).toBe('/projects/p1/work-items/t1')
    expect(legacyPathOf('/w/acme/s/p1/chat/c1')).toBe('/projects/p1/channels/c1')
    expect(legacyPathOf('/w/acme/s/p1/chat/archived')).toBe('/projects/p1/conversations/archived')
    expect(legacyPathOf('/w/acme/s/p1/planning/graph')).toBe('/projects/p1/graph')
    expect(legacyPathOf('/w/acme/s/p1/planning/import')).toBe('/projects/p1/plan-import')
    expect(legacyPathOf('/w/acme/s/p1/runs')).toBe('/projects/p1/launches')
    expect(legacyPathOf('/w/acme/settings/members')).toBe('/settings/workspace/members')
    expect(legacyPathOf('/w/acme/settings/spaces/p1/general')).toBe('/settings/projects/p1/general')
    expect(workspaceSlugOf('/w/acme/inbox')).toBe('acme')
    expect(workspaceSlugOf('/inbox')).toBeNull()
  })

  it('knows which old addresses are a workspace’s own pages', () => {
    for (const path of ['/home', '/inbox', '/runs/:runId', '/projects', '/projects/new', '/messages/x', '/dm/x', '/search?q=a', '/projects/p1', '/projects/:projectId/pages', '/settings/workspace/members', '/settings/projects/p1/general']) {
      expect(isWorkspacePath(path), path).toBe(true)
    }
    for (const path of ['/', '/settings/account', '/settings/*', '/AWD-12', '/:taskKey', '*', '/w/acme/home', '/sign-in', '/forms/:token']) {
      expect(isWorkspacePath(path), path).toBe(false)
    }
  })

  // W1.14b: the same grammar as the server's addresses module (addresses.rs `workspace_path`).
  it('moves a space’s pages under /w/<slug>/s/<id> with the address’s words', () => {
    const cases: [string, string][] = [
      ['/projects/p1', '/w/acme/s/p1'],
      ['/projects/p1/channels/c1', '/w/acme/s/p1/chat/c1'],
      ['/projects/p1/threads/t1', '/w/acme/s/p1/threads/t1'],
      ['/projects/p1/conversations/archived', '/w/acme/s/p1/chat/archived'],
      ['/projects/p1/conversations/channels', '/w/acme/s/p1/chat/channels'],
      ['/projects/p1/work-items', '/w/acme/s/p1/tasks'],
      ['/projects/p1/work-items/t1?tab=runs', '/w/acme/s/p1/tasks/t1?tab=runs'],
      ['/projects/p1/pages/x', '/w/acme/s/p1/pages/x'],
      ['/projects/p1/decisions#d1', '/w/acme/s/p1/decisions#d1'],
      ['/projects/p1/collections/c1?record=r1', '/w/acme/s/p1/collections/c1?record=r1'],
      ['/projects/p1/sheets/s1', '/w/acme/s/p1/sheets/s1'],
      ['/projects/p1/milestones', '/w/acme/s/p1/milestones'],
      ['/projects/p1/timeline', '/w/acme/s/p1/timeline'],
      ['/projects/p1/launches', '/w/acme/s/p1/runs'],
      ['/projects/p1/next', '/w/acme/s/p1/planning/next'],
      ['/projects/p1/suggestions', '/w/acme/s/p1/planning/suggestions'],
      ['/projects/p1/graph', '/w/acme/s/p1/planning/graph'],
      ['/projects/p1/plan-import', '/w/acme/s/p1/planning/import'],
      ['/projects/p1/settings', '/w/acme/settings/spaces/p1'],
      ['/settings/workspace/members', '/w/acme/settings/members'],
      ['/settings/projects/p1/automations', '/w/acme/settings/spaces/p1/automations'],
      ['/messages/c1', '/w/acme/dm/c1'],
    ]
    for (const [old, now] of cases) {
      expect(workspaceHref('acme', old), old).toBe(now)
      // Read back in today's words (an old space settings link reads back as the settings area's own).
      const back = old === '/projects/p1/settings' ? '/settings/projects/p1' : old.split(/[?#]/)[0]
      expect(legacyPathOf(now.split(/[?#]/)[0]), now).toBe(back)
    }
    expect(workspaceRoutePath('/projects/:projectId/work-items/:workItemId')).toBe('/s/:projectId/tasks/:workItemId')
    expect(workspaceRoutePath('/messages/:conversationId')).toBe('/messages/:conversationId')
  })

  it('builds space, settings and thing addresses in the workspace in view', () => {
    setCurrentWorkspaceSlug('acme')
    expect(spaceHref('p1')).toBe('/w/acme/s/p1')
    expect(spaceHref('p1', '/work-items/t1')).toBe('/w/acme/s/p1/tasks/t1')
    expect(spaceHref('p1', '/tasks?view=board')).toBe('/w/acme/s/p1/tasks?view=board')
    expect(spaceHref('p1', '?x=1')).toBe('/w/acme/s/p1?x=1')
    expect(spaceHref('p1', '', 'beta')).toBe('/w/beta/s/p1')
    expect(spaceSettingsHref('p1')).toBe('/w/acme/settings/spaces/p1/general')
    expect(thingHref('task', 't1')).toBe('/w/acme/t/task/t1')
    expect(hrefFor({ kind: 'task', id: 't1', spaceId: 'p1' })).toBe('/w/acme/s/p1/tasks/t1')
    expect(hrefFor({ kind: 'record', id: 'r1', spaceId: 'p1', parentId: 'c1' })).toBe('/w/acme/s/p1/collections/c1?record=r1')
    expect(hrefFor({ kind: 'whiteboard', id: 'b1' })).toBe('/w/acme/whiteboards/b1')
    expect(hrefFor({ kind: 'widget', id: 'x1' })).toBe('/w/acme/t/widget/x1')
    setCurrentWorkspaceSlug(null)
    expect(spaceHref('p1')).toBe('/w/-/s/p1')
  })

  it('finds the space in an address', () => {
    expect(spaceIdOf('/w/acme/s/p1/tasks')).toBe('p1')
    expect(spaceIdOf('/w/acme/settings/spaces/p1/general')).toBe('p1')
    expect(spaceIdOf('/projects/p1/work-items')).toBe('p1')
    expect(spaceIdOf('/projects/new')).toBeNull()
    expect(spaceIdOf('/w/acme/inbox')).toBeNull()
  })
})
