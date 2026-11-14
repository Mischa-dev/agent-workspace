import type { WebModule } from '../modules/define'

export default {
  id: 'pages',
  shell: 'project',
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
    { id: 'pages', space: 'project', rank: 20, part: 'area.pages', icon: 'list', label: { noun: 'page' }, path: '/pages', section: 'pages' },
    // Pages also holds Decisions: where a layout shows Decisions alone, the row opens them.
    { id: 'decisions', space: 'project', rank: 20, part: 'area.decisions', unless: 'area.pages', icon: 'list', label: 'Decisions', path: '/decisions', section: 'pages' },
    { id: 'pages', space: 'home', rank: 50, part: 'area.pages', icon: 'list', label: 'Pages', path: '/pages' },
  ],
  palette: [
    { id: 'p:pages', space: 'project', rank: 30, label: 'Pages', icon: 'list', shortcut: 'G P', keywords: ['docs', 'wiki'], path: '/pages' },
    { id: 'p:decisions', space: 'project', rank: 140, label: 'Decisions', icon: 'circle-check', shortcut: 'G D', path: '/decisions' },
  ],
} satisfies WebModule
