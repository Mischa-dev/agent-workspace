import type { PanelDocument } from './format'

/** A made-up panel with one of every block, for the pieces gallery (and the plan's sample). */
export const SAMPLE_PANEL: PanelDocument = {
  format: 'aw-panel/1',
  title: 'Builds',
  blocks: [
    { type: 'status', state: 'working', label: 'Build 214 is running', detail: 'Step 3 of 5: tests', progress: 0.6 },
    {
      type: 'fields',
      items: [
        { label: 'Branch', value: 'main' },
        { label: 'Started', value: '2026-11-14T09:12:00Z', kind: 'date' },
        { label: 'Result', value: { text: 'running', tone: 'agent' }, kind: 'badge' },
      ],
    },
    {
      type: 'table',
      columns: [
        { id: 'build', label: 'Build' },
        { id: 'branch', label: 'Branch' },
        { id: 'status', label: 'Status', kind: 'badge' },
      ],
      rows: [
        { id: '214', cells: { build: '214', branch: 'main', status: { text: 'running', tone: 'agent' } } },
        { id: '213', cells: { build: '213', branch: 'main', status: { text: 'passed', tone: 'success' } } },
        { id: '212', cells: { build: '212', branch: 'menus', status: { text: 'failed', tone: 'danger' } } },
      ],
      rowAction: { id: 'open_build' },
    },
    { type: 'chart', kind: 'bar', x: ['Mon', 'Tue', 'Wed', 'Thu', 'Fri'], series: [{ name: 'Failed builds', values: [2, 0, 1, 3, 0] }] },
    {
      type: 'section',
      title: 'Deploy',
      blocks: [
        { type: 'text', text: 'Deploys go out **one at a time**. Read [how deploys work](https://example.com/deploys) first.' },
        {
          type: 'form',
          id: 'deploy',
          schema: {
            type: 'object',
            properties: {
              target: { type: 'string', title: 'Where', enum: ['staging', 'production'] },
              note: { type: 'string', title: 'Note', description: 'Shown in the deploy log.' },
            },
            required: ['target'],
          },
          ui: { widgets: { note: 'textarea' } },
          submit: { label: 'Deploy', action: { id: 'deploy' } },
        },
      ],
    },
    {
      type: 'buttons',
      items: [
        { label: 'Rerun', action: { id: 'rerun' }, style: 'primary', confirm: { title: 'Rerun build 214?' } },
        { label: 'Cancel build', action: { id: 'cancel' }, style: 'danger', confirm: { title: 'Cancel build 214?' } },
      ],
    },
  ],
}
