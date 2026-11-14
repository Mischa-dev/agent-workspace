import type { WebModule } from '../modules/define'

export default {
  id: 'sheets',
  shell: 'project',
  routes: [{ path: '/projects/:projectId/sheets/*', space: 'project', page: () => import('./SheetsArea') }],
  sidebar: [{ id: 'sheets', space: 'project', rank: 40, part: 'area.sheets', icon: 'table', label: 'Sheets', path: '/sheets' }],
  palette: [
    { id: 'p:sheets', space: 'project', rank: 50, label: 'Sheets', icon: 'table', keywords: ['spreadsheet', 'formulas', 'cells', 'budget'], path: '/sheets' },
  ],
} satisfies WebModule
