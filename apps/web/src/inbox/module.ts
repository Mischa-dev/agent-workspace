import type { WebModule } from '../modules/define'

export default {
  id: 'inbox',
  shell: 'home',
  routes: [
    { path: '/inbox', space: 'home', page: () => import('./InboxPage').then((module) => ({ default: module.InboxPage })) },
    { path: '/later', space: 'home', page: () => import('../later/LaterPage').then((module) => ({ default: module.LaterPage })) },
  ],
  sidebar: [
    // Inbox shows wherever its module is on: no layout hides it.
    { id: 'inbox', space: 'home', rank: 10, icon: 'inbox', label: 'Inbox', path: '/inbox', badge: 'inbox' },
    { id: 'later', space: 'home', rank: 30, part: 'workspace.later', icon: 'bookmark', label: 'Later', path: '/later' },
  ],
  palette: [
    {
      id: 'g:inbox',
      space: 'workspace',
      rank: 10,
      label: 'Inbox',
      icon: 'inbox',
      shortcut: 'G I',
      keywords: ['notifications', 'mentions', 'needs you'],
      path: '/inbox',
    },
    {
      id: 'g:later',
      space: 'workspace',
      rank: 30,
      label: 'Later',
      icon: 'bookmark',
      shortcut: 'G L',
      keywords: ['saved', 'bookmarks', 'reminders', 'scheduled'],
      path: '/later',
    },
  ],
  phone: true,
} satisfies WebModule
