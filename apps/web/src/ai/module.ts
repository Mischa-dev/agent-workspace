import type { WebModule } from '../modules/define'

export default {
  id: 'suggestions',
  shell: 'project',
  routes: [
    {
      path: '/projects/:projectId/suggestions',
      space: 'project',
      page: () => import('./PlanningPages').then((module) => ({ default: module.SuggestionsPage })),
    },
  ],
  palette: [
    {
      id: 'p:suggestions',
      space: 'project',
      rank: 110,
      label: 'Suggestions',
      icon: 'bot',
      keywords: ['dependencies', 'blockers', 'review', 'triage'],
      path: '/suggestions',
    },
  ],
} satisfies WebModule
