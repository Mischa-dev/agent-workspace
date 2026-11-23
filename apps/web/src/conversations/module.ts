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
      path: '/projects/:projectId/conversations/channels',
      space: 'project',
      page: () => import('./ChannelBrowserPage').then((module) => ({ default: module.ChannelBrowserPage })),
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
    { id: 'p:channels', space: 'project', rank: 149, label: 'Browse channels', icon: 'hash', path: '/conversations/channels' },
    { id: 'p:archived', space: 'project', rank: 150, label: 'Archived conversations', icon: 'archive', path: '/conversations/archived' },
  ],
  things: [
    { kind: 'channel', label: 'Channel', icon: 'message' },
    { kind: 'thread', label: 'Thread', icon: 'message' },
  ],
  embeds: [
    {
      // One thread: /projects/{projectId}/threads/{id}.
      id: 'thread',
      match: (path) => {
        const found = /^\/projects\/[^/]+\/threads\/([^/]+)\/?$/.exec(path)
        return found ? { kind: 'thread', id: decodeURIComponent(found[1]) } : null
      },
      view: () => import('../pieces/embeds/ThingEmbedView'),
      loading: 'the thread',
    },
  ],
  phone: { id: 'chat', space: 'project', rank: 20, part: 'area.chat', icon: 'hash', label: 'Chat', path: '' },
} satisfies WebModule
