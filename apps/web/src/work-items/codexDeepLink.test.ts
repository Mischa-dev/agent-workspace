import { describe, expect, it } from 'vitest'

import type { WorkItem } from './types'
import { buildOpenInCodexLink } from './codexDeepLink'
import { EMPTY_PLANNING } from './testFixtures'

function workItem(overrides: Partial<WorkItem> = {}): WorkItem {
  return {
    id: 'item/id', projectId: 'project id', key: 'AW-7', number: 7,
    title: 'Test', description: '', itemType: { id: 'type', name: 'Task' },
    status: { id: 'status', name: 'Open', semanticCategory: 'backlog' },
    priority: 'medium', assignee: null, version: 1, archivedAt: null,
    createdAt: '', updatedAt: '', labels: [], acceptanceCriteria: [], linkedThreads: [],
    linkedPullRequests: [], capabilities: { view: true, edit: true, archive: true },
    linkedRepositories: [{ id: 'repo', owner: 'owner', name: 'repo', cloneUrl: 'https://github.com/owner/repo.git', role: 'primary', preparationState: 'ready', trusted: true, available: true, archivedAt: null, href: '' }],
    ...EMPTY_PLANNING,
    ...overrides,
  }
}

describe('buildOpenInCodexLink', () => {
  it('builds an encoded deep link with canonical workspace context', () => {
    const result = buildOpenInCodexLink(workItem(), 'https://workspace.example/')
    expect(result.reason).toBeNull()
    const url = new URL(result.href!)
    expect(url.protocol).toBe('codex:')
    expect(url.searchParams.get('originUrl')).toBe('https://github.com/owner/repo.git')
    expect(url.searchParams.get('prompt')).toContain('Project URL: https://workspace.example/projects/project%20id')
    expect(url.searchParams.get('prompt')).toContain('Work Item URL: https://workspace.example/projects/project%20id/work-items/item%2Fid')
  })

  it.each([
    ['an archived item', workItem({ archivedAt: 'now' }), 'Restore this task'],
    ['no repository', workItem({ linkedRepositories: [] }), 'Link one trusted repository'],
    ['multiple repositories', workItem({ linkedRepositories: [workItem().linkedRepositories[0], workItem().linkedRepositories[0]] }), 'exactly one'],
    ['an untrusted repository', repositoryOverride({ trusted: false }), 'must be trusted'],
    ['an archived repository', repositoryOverride({ archivedAt: 'now' }), 'Restore the linked repository'],
    ['an unavailable repository', repositoryOverride({ available: false }), 'Prepare and enable'],
  ])('refuses %s', (_label, item, reason) => {
    expect(buildOpenInCodexLink(item, 'https://workspace.example')).toEqual({ href: null, reason: expect.stringContaining(reason) })
  })

  it.each([
    'http://github.com/owner/repo.git',
    'https://github.com/owner/repo',
    'https://github.com/owner/repo.git/extra',
    'https://evil.example/owner/repo.git',
    'https://github.com/owner/repo.git?token=secret',
  ])('rejects a non-canonical or unsafe Git origin: %s', (cloneUrl) => {
    const result = buildOpenInCodexLink(repositoryOverride({ cloneUrl }), 'https://workspace.example')
    expect(result.href).toBeNull()
    expect(result.reason).toContain('supported canonical Git origin URL')
  })
})

function repositoryOverride(repository: Partial<WorkItem['linkedRepositories'][number]>) {
  const item = workItem()
  return workItem({ linkedRepositories: [{ ...item.linkedRepositories[0], ...repository }] })
}
