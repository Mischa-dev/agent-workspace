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
  ],
  palette: [
    { id: 'g:projects', space: 'workspace', rank: 70, label: 'All projects', icon: 'list', keywords: ['projects', 'archived'], path: '/projects' },
  ],
} satisfies WebModule
