import type { WebModule } from '../modules/define'

export default {
  id: 'runs',
  shell: 'home',
  routes: [
    { path: '/runs', space: 'home', page: () => import('./RunsPage').then((module) => ({ default: module.RunsPage })) },
    { path: '/runs/:runId', space: 'home', page: () => import('./RunPage').then((module) => ({ default: module.RunPage })) },
    { path: '/projects/:projectId/launches', space: 'project', page: () => import('./RunsPage').then((module) => ({ default: module.ProjectRunsPage })) },
  ],
  sidebar: [
    { id: 'runs', space: 'project', rank: 50, part: 'area.runs', icon: 'activity', label: 'Runs', path: '/launches' },
    { id: 'runs', space: 'home', rank: 40, part: 'workspace.runs', icon: 'activity', label: 'Runs', path: '/runs' },
  ],
  palette: [
    { id: 'p:runs', space: 'project', rank: 100, label: 'Runs', icon: 'activity', keywords: ['launches', 'launch queue', 'agent runs'], path: '/launches' },
    { id: 'g:runs', space: 'workspace', rank: 40, label: 'Runs', icon: 'activity', keywords: ['agent runs', 'tokens', 'cost', 'usage', 'diff'], path: '/runs' },
  ],
} satisfies WebModule
