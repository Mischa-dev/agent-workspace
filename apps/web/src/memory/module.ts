import type { WebModule } from '../modules/define'

export default {
  id: 'pages',
  shell: 'project',
  publicRoutes: [{ path: '/p/:token', space: 'none', layout: 'public', page: () => import('./PublicPage') }],
  routes: [
    { path: '/pages', space: 'workspace', page: () => import('./MemoryPages').then((module) => ({ default: module.PagesPage })) },
    { path: '/projects/:projectId/pages', space: 'project', page: () => import('./MemoryPages').then((module) => ({ default: module.PagesPage })) },
    {
      path: '/projects/:projectId/pages/:pageId',
      space: 'project',
      page: () => import('./MemoryPages').then((module) => ({ default: module.PageDetailPage })),
    },
    {
      path: '/projects/:projectId/decisions',
      space: 'project',
      page: () => import('./MemoryPages').then((module) => ({ default: module.DecisionsPage })),
    },
  ],
  sidebar: [
    { id: 'pages', space: 'project', rank: 20, part: 'area.pages', unless: 'area.docs', icon: 'list', label: { noun: 'page' }, path: '/pages', section: 'pages' },
    // Where a layout shows Docs, that one row holds pages with every other kind of document.
    // Pages also holds Decisions: where a layout shows Decisions alone, the row opens them.
    { id: 'decisions', space: 'project', rank: 20, part: 'area.decisions', unless: 'area.pages', icon: 'list', label: 'Decisions', path: '/decisions', section: 'pages' },
    { id: 'pages', space: 'home', rank: 50, part: 'area.pages', icon: 'list', label: 'Pages', path: '/pages' },
  ],
  palette: [
    { id: 'p:pages', space: 'project', rank: 30, label: 'Pages', icon: 'list', shortcut: 'G P', keywords: ['docs', 'wiki'], path: '/pages' },
    { id: 'p:decisions', space: 'project', rank: 140, label: 'Decisions', icon: 'circle-check', shortcut: 'G D', path: '/decisions' },
  ],
  shortcuts: [
    { action: 'go-pages', key: 'p', label: 'Pages', path: '/pages', inProject: true, rank: 80 },
    { action: 'go-decisions', key: 'd', label: 'Decisions', path: '/decisions', inProject: true, rank: 90 },
  ],
  things: [
    { kind: 'page', label: 'Page', icon: 'list' },
    { kind: 'decision', label: 'Decision', icon: 'circle-check' },
  ],
  embeds: [
    {
      // One page: /projects/{projectId}/pages/{id}.
      id: 'page',
      match: (path) => {
        const found = /^\/projects\/[^/]+\/pages\/([^/]+)\/?$/.exec(path)
        return found ? { kind: 'page', id: decodeURIComponent(found[1]) } : null
      },
      view: () => import('../pieces/embeds/ThingEmbedView'),
      loading: 'the page',
    },
    {
      // A file of the page (C1.33), such as a pasted picture: /files/{fileId}.
      id: 'file',
      match: (path) => {
        const found = /^\/files\/([0-9a-f-]{36})\/?$/i.exec(path)
        return found ? { fileId: found[1].toLowerCase() } : null
      },
      view: () => import('../files/FileEmbedView'),
      loading: 'the file',
    },
  ],
} satisfies WebModule
