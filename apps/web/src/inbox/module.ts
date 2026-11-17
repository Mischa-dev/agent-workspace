import type { WebModule } from '../modules/define'

export default {
  id: 'inbox',
  shell: 'home',
  routes: [
    { path: '/inbox', space: 'home', page: () => import('./InboxPage').then((module) => ({ default: module.InboxPage })) },
    { path: '/later', space: 'home', page: () => import('../later/LaterPage').then((module) => ({ default: module.LaterPage })) },
    { path: '/threads', space: 'home', page: () => import('./ThreadsPage').then((module) => ({ default: module.ThreadsPage })) },
  ],
  sidebar: [
    // Inbox shows wherever its module is on: no layout hides it.
    { id: 'inbox', space: 'home', rank: 10, icon: 'inbox', label: 'Inbox', path: '/inbox', badge: 'inbox' },
    { id: 'threads', space: 'home', rank: 20, part: 'workspace.threads', icon: 'messages', label: 'Threads', path: '/threads' },
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
      id: 'g:threads',
      space: 'workspace',
      rank: 20,
      label: 'Threads',
      icon: 'messages',
      keywords: ['followed', 'replies', 'discussions'],
      path: '/threads',
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
  capture: [{ id: 'reminder', label: 'New reminder', icon: 'bookmark', path: '/later?new=reminder' }],
  shortcuts: [
    { action: 'go-inbox', key: 'i', label: 'Inbox', path: '/inbox', rank: 20 },
    { action: 'go-later', key: 'l', label: 'Later', path: '/later', rank: 40 },
  ],
  phone: true,
} satisfies WebModule
