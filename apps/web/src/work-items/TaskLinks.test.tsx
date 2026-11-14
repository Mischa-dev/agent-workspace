import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { TaskLinks } from './TaskLinks'
import type { LinkedPullRequest, WorkItem } from './types'

const mutate = vi.fn()
const merge = vi.fn()
vi.mock('../conversations/hooks', () => ({ useProjectConversationSidebar: () => ({ data: undefined }) }))
vi.mock('./hooks', () => {
  const idle = () => ({ mutate: vi.fn(), isPending: false, error: null })
  return {
    useSetWorkItemPrimaryRepository: idle,
    useSetWorkItemRepositoryLink: idle,
    useSetWorkItemThreadLink: idle,
    useSetWorkItemPullRequestLink: () => ({ mutate, isPending: false, error: null }),
    useMergeWorkItemPullRequest: () => ({ mutate: merge, reset: vi.fn(), isPending: false, error: null }),
  }
})

function pullRequest(extra: Partial<LinkedPullRequest>): LinkedPullRequest {
  return {
    id: 'pr-1',
    number: 59,
    title: 'PR card',
    url: 'https://github.com/o/r/pull/59',
    state: 'open',
    draft: false,
    source: 'branch_key',
    confirmed: false,
    checksState: 'failure',
    reviewState: 'pending',
    mergeabilityState: 'blocked',
    syncState: 'fresh',
    syncFailureCategory: null,
    automationState: 'not_evaluated',
    automationReason: null,
    headRef: 'awd-59-card',
    headSha: 'a'.repeat(40),
    baseRef: 'main',
    authorLogin: 'mischa',
    requestedReviewers: [],
    checks: [
      { name: 'lint', status: 'completed', conclusion: 'failure', url: 'https://github.com/checks/1' },
      ...Array.from({ length: 6 }, (_, index) => ({ name: `test ${index + 1}`, status: 'completed', conclusion: 'success', url: null })),
    ],
    ...extra,
  }
}

function draw(linked: LinkedPullRequest, mergePullRequests = false) {
  const item = {
    id: 'task-1',
    projectId: 'p1',
    version: 4,
    capabilities: { view: true, edit: true, archive: true, mergePullRequests },
    linkedThreads: [],
    linkedRepositories: [],
    linkedPullRequests: [linked],
  } as unknown as WorkItem
  render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter>
        <TaskLinks adding={false} disabled={false} item={item} onAdding={() => undefined} />
      </MemoryRouter>
    </QueryClientProvider>,
  )
}

afterEach(() => {
  cleanup()
  mutate.mockReset()
  merge.mockReset()
})

describe('a task’s pull requests', () => {
  it('says why one is suggested, and links or dismisses it', () => {
    draw(pullRequest({}))
    expect(screen.getByText(/suggested because it names this task/)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Link' }))
    expect(mutate.mock.calls[0][0]).toMatchObject({ pullRequestId: 'pr-1', linked: true, expectedVersion: 4 })
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss #59' }))
    expect(mutate.mock.calls[1][0]).toMatchObject({ pullRequestId: 'pr-1', linked: false })
  })

  it('lists its checks, failing first, and the rest on request', () => {
    draw(pullRequest({ confirmed: true }))
    expect(screen.queryByRole('button', { name: 'Link' })).toBeNull()
    const checks = screen.getByRole('list', { name: 'Checks on #59' })
    expect(checks.querySelectorAll('li')).toHaveLength(5)
    expect(checks.querySelector('li')?.textContent).toContain('lint')
    expect(checks.querySelector('li')?.textContent).toContain('failure')
    expect(screen.getByText('awd-59-card → main · by mischa')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Show 2 more checks' }))
    expect(checks.querySelectorAll('li')).toHaveLength(7)
  })

  it('merges a linked pull request after one confirmation, for those who manage code', () => {
    draw(pullRequest({ confirmed: true }))
    expect(screen.queryByRole('button', { name: 'Merge' })).toBeNull()
    cleanup()
    draw(pullRequest({ confirmed: true, draft: true }), true)
    expect(screen.queryByRole('button', { name: 'Merge' })).toBeNull()
    cleanup()
    draw(pullRequest({ confirmed: true }), true)
    fireEvent.click(screen.getByRole('button', { name: 'Merge' }))
    expect(merge).not.toHaveBeenCalled()
    expect(screen.getByText('Merge #59?')).toBeTruthy()
    const buttons = screen.getAllByRole('button', { name: 'Merge' })
    fireEvent.click(buttons[buttons.length - 1])
    expect(merge.mock.calls[0][0]).toMatchObject({ pullRequestId: 'pr-1', expectedHeadSha: 'a'.repeat(40) })
  })
})
