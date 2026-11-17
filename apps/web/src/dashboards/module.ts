import type { WebModule } from '../modules/define'

export default {
  id: 'dashboards',
  shell: 'project',
  routes: [{ path: '/projects/:projectId/dashboards/*', space: 'project', page: () => import('./DashboardsArea') }],
  sidebar: [{ id: 'dashboards', space: 'project', rank: 35, part: 'area.dashboards', icon: 'chart', label: 'Dashboards', path: '/dashboards' }],
  palette: [
    {
      id: 'p:dashboards',
      space: 'project',
      rank: 45,
      label: 'Dashboards',
      icon: 'chart',
      keywords: ['charts', 'reports', 'figures', 'metrics'],
      path: '/dashboards',
    },
  ],
  things: [{ kind: 'dashboard', label: 'Dashboard', icon: 'chart' }],
} satisfies WebModule
