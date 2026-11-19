import type { WebModule } from '../modules/define'

export default {
  id: 'sheets',
  shell: 'project',
  routes: [{ path: '/projects/:projectId/sheets/*', space: 'project', page: () => import('./SheetsArea') }],
  sidebar: [{ id: 'sheets', space: 'project', rank: 40, part: 'area.sheets', icon: 'table', label: 'Sheets', path: '/sheets' }],
  palette: [
    { id: 'p:sheets', space: 'project', rank: 50, label: 'Sheets', icon: 'table', keywords: ['spreadsheet', 'formulas', 'cells', 'budget'], path: '/sheets' },
  ],
  things: [{ kind: 'sheet', label: 'Sheet', icon: 'table' }],
  embeds: [
    {
      // A sheet's range: /projects/{projectId}/sheets/{sheetId}, with ?tab= and ?range=, or ?name= for a named range.
      id: 'sheet-range',
      match: (path, search) => {
        const found = /^\/projects\/([^/]+)\/sheets\/([^/]+)\/?$/.exec(path)
        return found
          ? {
              projectId: decodeURIComponent(found[1]),
              sheetId: decodeURIComponent(found[2]),
              tabId: search.get('tab') || null,
              range: search.get('range') || null,
              name: search.get('name') || null,
            }
          : null
      },
      view: () => import('./SheetEmbedView'),
      loading: 'the sheet',
      picker: { label: 'Sheet…', part: 'area.sheets', view: () => import('./SheetEmbedPicker') },
    },
  ],
  live: [{ events: ['sheet.updated', 'sheet.ranges_changed'], handler: () => import('./live') }],
} satisfies WebModule
