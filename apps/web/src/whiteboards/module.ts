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
  things: [{ kind: 'whiteboard', label: 'Whiteboard', icon: 'whiteboard' }],
  embeds: [
    {
      // One whiteboard: /whiteboards/{id}.
      id: 'whiteboard',
      match: (path) => {
        const found = /^\/whiteboards\/([^/]+)\/?$/.exec(path)
        return found ? { kind: 'whiteboard', id: decodeURIComponent(found[1]) } : null
      },
      view: () => import('../pieces/embeds/ThingEmbedView'),
      loading: 'the whiteboard',
    },
  ],
} satisfies WebModule
