import type { WebModule } from '../modules/define'

export default {
  id: 'collections',
  shell: 'project',
  routes: [{ path: '/projects/:projectId/collections/*', space: 'project', page: () => import('./CollectionsArea') }],
  publicRoutes: [{ path: '/forms/:token', space: 'none', layout: 'public', page: () => import('../forms/Form') }],
  sidebar: [{ id: 'collections', space: 'project', rank: 30, part: 'area.collections', unless: 'area.docs', icon: 'table', label: 'Collections', path: '/collections' }],
  palette: [
    {
      id: 'p:collections',
      space: 'project',
      rank: 40,
      label: 'Collections',
      icon: 'table',
      keywords: ['records', 'tables', 'database', 'spreadsheet'],
      path: '/collections',
    },
  ],
  things: [{ kind: 'record', label: 'Record', icon: 'table' }],
  embeds: [
    {
      // A collection's view: /projects/{projectId}/collections/{collectionId}, with ?view= for a saved view.
      id: 'collection-view',
      match: (path, search) => {
        const found = /^\/projects\/([^/]+)\/collections\/([^/]+)\/?$/.exec(path)
        return found
          ? { projectId: decodeURIComponent(found[1]), collectionId: decodeURIComponent(found[2]), viewId: search.get('view') || null }
          : null
      },
      view: () => import('./CollectionEmbedView'),
      loading: 'the collection',
      picker: { label: 'Collection view…', part: 'area.collections', view: () => import('./CollectionEmbedPicker') },
    },
  ],
  live: [{ events: ['collection.updated'], handler: () => import('./live') }],
} satisfies WebModule
