import type { WebModule } from '../modules/define'

export default {
  id: 'channels',
  shell: 'project',
  routes: [
    {
      path: '/projects/:projectId/channels/:channelId',
      space: 'project',
      page: () => import('./ChannelPage').then((module) => ({ default: module.ChannelPage })),
    },
    {
      path: '/projects/:projectId/conversations/archived',
      space: 'project',
      page: () => import('./ConversationArchivePage').then((module) => ({ default: module.ConversationArchivePage })),
    },
    {
      path: '/projects/:projectId/threads/:threadId',
      space: 'project',
      page: () => import('../threads/ThreadPage').then((module) => ({ default: module.ThreadPage })),
    },
  ],
  palette: [
    { id: 'p:archived', space: 'project', rank: 150, label: 'Archived conversations', icon: 'archive', path: '/conversations/archived' },
  ],
  phone: true,
} satisfies WebModule
