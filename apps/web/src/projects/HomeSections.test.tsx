import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { WorkingNow, YourTasks } from './HomeSections'

vi.mock('../experience/layout', () => ({
  useHomeExperience: () => ({ shows: () => true, word: (_word: string, form: string) => (form === 'many' ? 'tasks' : 'task') }),
}))

vi.mock('./homeApi', () => ({
  homeKeys: { summary: ['home', 'summary'] },
  getHome: async () => ({
    working: [
      {
        id: 'run-1',
        state: 'working',
        projectId: 'p1',
        projectName: 'Agent Workspace',
        threadId: 't1',
        threadName: 'Fix the board',
        task: { id: 'task-1', key: 'AWD-7', title: 'Fix the board' },
        harness: 'claude_code',
        harnessName: 'Claude Code',
        model: 'model-x',
        startedBy: 'Ana',
        mine: false,
        approvalsWaiting: 1,
        createdAt: new Date().toISOString(),
        startedAt: new Date().toISOString(),
        href: '/runs/run-1',
      },
    ],
    workingTotal: 1,
    queuedLaunches: 0,
  }),
}))

vi.mock('../views/api', () => ({
  viewKeys: { myWork: ['my-work'] },
  getMyWork: async () => ({
    assigned: {
      items: [
        { id: 'task-2', key: 'AWD-8', title: 'Write the guide', projectId: 'p1', projectName: 'Agent Workspace', statusName: 'In Progress', priority: 'high', overdue: true, blocked: false },
        { id: 'task-3', key: 'AWD-9', title: 'Tidy up', projectId: 'p1', projectName: 'Agent Workspace', statusName: 'Backlog', priority: 'no_priority', overdue: false, blocked: false },
      ],
      total: 2,
    },
    claimed: { items: [], total: 0 },
    review: { items: [], total: 0 },
  }),
}))

function draw(node: React.ReactNode) {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter>{node}</MemoryRouter>
    </QueryClientProvider>,
  )
}

afterEach(cleanup)

describe("Home's cards on the pieces", () => {
  it('Working now reads a run: its task with key, what it waits on, where, and who started it when', async () => {
    draw(<WorkingNow />)
    const link = await screen.findByRole('link', { name: 'AWD-7 Fix the board' })
    expect(link.getAttribute('href')).toBe('/runs/run-1')
    const row = link.closest('li')!
    expect(row.querySelector('.aw-chip--warning')?.textContent).toBe('Needs approval')
    expect(row.textContent).toContain('Agent Workspace')
    expect(row.textContent).toContain('Claude Code')
    expect(row.textContent).toContain('Started')
    expect(row.textContent).toContain('by Ana')
  })

  it('Your tasks flags what needs attention and leaves out an empty priority', async () => {
    draw(<YourTasks />)
    const first = (await screen.findByRole('link', { name: 'AWD-8 Write the guide' })).closest('li')!
    expect(first.querySelector('.aw-chip--danger')?.textContent).toBe('Overdue')
    expect(first.textContent).toContain('In Progress')
    expect(first.textContent).toContain('High')
    const second = screen.getByRole('link', { name: 'AWD-9 Tidy up' }).closest('li')!
    expect(second.querySelector('.aw-chip--danger, .aw-chip--warning')).toBeNull()
    expect(second.textContent).not.toContain('None')
    expect(second.textContent).not.toContain('No priority')
  })
})
