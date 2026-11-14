import type { WebModule } from '../modules/define'

export default {
  id: 'whiteboards',
  shell: 'home',
  routes: [
    { path: '/whiteboards', space: 'home', page: () => import('./WhiteboardsPage').then((module) => ({ default: module.WhiteboardsPage })) },
    {
      path: '/whiteboards/:whiteboardId',
      space: 'home',
      layout: 'canvas',
      page: () => import('./WhiteboardPage').then((module) => ({ default: module.WhiteboardPage })),
    },
  ],
  sidebar: [{ id: 'whiteboards', space: 'home', rank: 60, part: 'area.whiteboards', icon: 'whiteboard', label: 'Whiteboards', path: '/whiteboards' }],
  palette: [
    {
      id: 'g:whiteboards',
      space: 'workspace',
      rank: 50,
      label: 'Whiteboards',
      icon: 'whiteboard',
      keywords: ['excalidraw', 'draw', 'diagram', 'sketch', 'canvas'],
      path: '/whiteboards',
    },
  ],
} satisfies WebModule
