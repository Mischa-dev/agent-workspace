import type { WebModule } from '../modules/define'

export default {
  id: 'projects',
  shell: 'project',
  routes: [
    { path: '/projects', space: 'home', page: () => import('./ProjectsPage').then((module) => ({ default: module.ProjectsPage })) },
    { path: '/projects/new', space: 'workspace', page: () => import('./CreateProjectPage').then((module) => ({ default: module.CreateProjectPage })) },
    {
      path: '/projects/:projectId',
      space: 'project',
      page: () => import('./ProjectOverviewPage').then((module) => ({ default: module.ProjectOverviewPage })),
    },
    // Docs: every kind of document the project holds, in one tree with folders.
    { path: '/projects/:projectId/docs', space: 'project', page: () => import('../docs/DocsPage') },
  ],
  sidebar: [{ id: 'docs', space: 'project', rank: 20, part: 'area.docs', icon: 'folder', label: 'Docs', path: '/docs' }],
  capture: [{ id: 'project', label: 'New project', icon: 'plus', path: '/projects/new' }],
  palette: [
    {
      id: 'p:docs',
      space: 'project',
      rank: 29,
      label: 'Docs',
      icon: 'folder',
      keywords: ['folders', 'files', 'pages', 'sheets', 'whiteboards', 'collections', 'dashboards'],
      path: '/docs',
    },
    { id: 'g:projects', space: 'workspace', rank: 70, label: 'All projects', icon: 'list', keywords: ['projects', 'archived'], path: '/projects' },
  ],
} satisfies WebModule
