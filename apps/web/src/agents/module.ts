import type { WebModule } from '../modules/define'

export default {
  id: 'agents',
  shell: 'none',
  routes: [
    { path: '/messages/agents', space: 'direct', page: () => import('../direct/MessagesPage').then((module) => ({ default: module.AgentsPage })) },
  ],
  palette: [{ id: 'g:agents', space: 'workspace', rank: 80, label: 'Agents', icon: 'bot', path: '/messages/agents' }],
} satisfies WebModule
