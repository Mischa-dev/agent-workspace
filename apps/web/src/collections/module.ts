import type { WebModule } from '../modules/define'

export default {
  id: 'collections',
  shell: 'project',
  routes: [{ path: '/projects/:projectId/collections/*', space: 'project', page: () => import('./CollectionsArea') }],
  publicRoutes: [{ path: '/forms/:token', space: 'none', layout: 'public', page: () => import('../forms/Form') }],
  sidebar: [{ id: 'collections', space: 'project', rank: 30, part: 'area.collections', icon: 'table', label: 'Collections', path: '/collections' }],
  palette: [
    {
      id: 'p:collections',
      space: 'project',
      rank: 40,
      label: 'Collections',
      icon: 'table',
      keywords: ['records', 'tables', 'database', 'spreadsheet'],
      path: '/collections',
    },
  ],
} satisfies WebModule
