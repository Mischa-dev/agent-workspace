import { QueryClient } from '@tanstack/react-query'
import { describe, expect, it } from 'vitest'

import { layoutKeys } from '../experience/layout'
import { modulesChanged } from './moduleSwitches'
import type { RealtimeEnvelope } from './realtimeModel'

const WORKSPACE = '0190f5a4-0000-7000-8000-000000000010'
const SPACE = '0190f5a4-0000-7000-8000-000000000001'
const OTHER = '0190f5a4-0000-7000-8000-000000000002'

function envelope(type: string, payload: Record<string, unknown>): RealtimeEnvelope {
  return {
    eventId: '0190f5a4-0000-7000-8000-0000000000ff',
    eventCursor: 1,
    version: 1,
    projectId: SPACE,
    conversationId: null,
    type,
    occurredAt: '2026-11-16T00:00:00Z',
    payload,
  }
}

describe('a space’s module switches from the realtime socket', () => {
  it('refetches the project list, that space’s layout and Settings › Modules, nothing else', () => {
    const queryClient = new QueryClient()
    const keys = [
      ['projects'],
      layoutKeys.project(SPACE),
      layoutKeys.project(OTHER),
      layoutKeys.workspace(WORKSPACE),
      ['organization', WORKSPACE, 'modules'],
    ]
    for (const key of keys) queryClient.setQueryData(key, { cached: true })
    const changed = modulesChanged(
      queryClient,
      envelope('project.modules.updated', { workspaceId: WORKSPACE, projectId: SPACE, changed: { whiteboards: false } }),
    )
    expect(changed).toBe(true)
    const stale = (key: readonly unknown[]) => queryClient.getQueryState(key)?.isInvalidated
    expect(stale(['projects'])).toBe(true)
    expect(stale(layoutKeys.project(SPACE))).toBe(true)
    expect(stale(['organization', WORKSPACE, 'modules'])).toBe(true)
    expect(stale(layoutKeys.project(OTHER))).toBe(false)
    expect(stale(layoutKeys.workspace(WORKSPACE))).toBe(false)
  })

  it('leaves other events alone', () => {
    const queryClient = new QueryClient()
    queryClient.setQueryData(layoutKeys.project(SPACE), { cached: true })
    expect(modulesChanged(queryClient, envelope('page.updated', { pageId: OTHER, version: 1 }))).toBe(false)
    expect(queryClient.getQueryState(layoutKeys.project(SPACE))?.isInvalidated).toBe(false)
  })
})
