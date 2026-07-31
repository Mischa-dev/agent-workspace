import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, render, screen, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { Project } from '../projects/types'
import { ProjectSidebar } from './ProjectSidebar'

const projectId = '019fb600-0000-7000-8000-000000000001'
const channelId = '019fb600-0000-7000-8000-000000000002'

const project: Project = {
  id: projectId,
  organizationId: '019fb600-0000-7000-8000-000000000003',
  key: 'AW',
  name: 'Agent Workspace',
  archivedAt: null,
  role: 'owner',
  version: 1,
  attentionCount: 5,
  capabilities: {
    view: true,
    manage: true,
    archive: true,
    navigationWrite: true,
  },
  navigation: {
    location: { kind: 'channel', conversationId: channelId },
    href: `/projects/${projectId}/channels/${channelId}`,
    pinned: false,
    lastOpenedAt: '2026-07-31T15:00:00Z',
    version: 1,
  },
}

const states = [
  ['response_required', 'Needs your response'],
  ['active', 'Run active'],
  ['failed', 'Run failed'],
  ['completed', 'Run completed'],
  ['unread', 'Unread Messages'],
  ['settled', 'Seen'],
] as const

const stateIcons = {
  response_required: '!',
  active: '↻',
  failed: '×',
  completed: '✓',
  unread: '●',
  settled: '·',
} as const

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

describe('Project Conversation sidebar', () => {
  it('renders a visible text and icon signal for every Thread attention state', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() =>
        Promise.resolve(
          new Response(
            JSON.stringify({
              projectId,
              channels: [
                {
                  id: channelId,
                  name: 'General',
                  archivedAt: null,
                  highestSequence: 4,
                  highestSeenSequence: 4,
                  unread: false,
                  threads: states.map(([attention, label], index) => ({
                    id: `019fb600-0000-7000-8000-${String(index + 10).padStart(12, '0')}`,
                    parentChannelId: channelId,
                    originMessageId: null,
                    name: `Thread ${index + 1}`,
                    archivedAt: null,
                    parentArchivedAt: null,
                    highestSequence: 1,
                    highestSeenSequence: attention === 'unread' ? 0 : 1,
                    attention: { kind: attention, label },
                  })),
                  hasMoreThreads: false,
                },
              ],
              capabilities: { createChannel: false },
            }),
            { headers: { 'content-type': 'application/json' } },
          ),
        ),
      ),
    )
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    })
    render(
      <QueryClientProvider client={queryClient}>
        <MemoryRouter
          initialEntries={[
            `/projects/${projectId}/channels/${channelId}`,
          ]}
        >
          <ProjectSidebar
            mobileOpen={false}
            onNavigate={vi.fn()}
            project={project}
            selectedChannelId={channelId}
          />
        </MemoryRouter>
      </QueryClientProvider>,
    )

    for (const [attention, label] of states) {
      const visibleLabel = await screen.findByText(label)
      expect(visibleLabel).toBeVisible()
      expect(visibleLabel).not.toHaveClass('visually-hidden')
      const status = visibleLabel.closest('[data-attention]')
      expect(status).toHaveAttribute('data-attention', attention)
      const icon = within(status as HTMLElement).getByText(
        stateIcons[attention],
      )
      expect(icon).toHaveAttribute('aria-hidden', 'true')
      expect(icon).toHaveClass('thread-attention-icon')
    }
  })
})
