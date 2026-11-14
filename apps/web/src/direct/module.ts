import type { WebModule } from '../modules/define'

export default {
  id: 'messages',
  shell: 'none',
  routes: [
    { path: '/messages', space: 'direct', page: () => import('./MessagesPage').then((module) => ({ default: module.MessagesPage })) },
    {
      path: '/messages/:conversationId',
      space: 'direct',
      page: () => import('./DirectConversationPage').then((module) => ({ default: module.DirectConversationPage })),
    },
  ],
  palette: [
    { id: 'g:messages', space: 'workspace', rank: 60, label: 'Messages', icon: 'message', shortcut: 'G M', keywords: ['dms', 'direct messages', 'groups'], path: '/messages' },
  ],
  phone: true,
} satisfies WebModule
