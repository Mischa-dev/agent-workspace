import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { renderPiece } from '../../pieces/testing'
import type { Project } from '../../projects/types'
import { BuiltModules } from './BuiltModules'

const callOp = vi.fn()
vi.mock('../../api/ops', async (original) => ({
  ...(await original<typeof import('../../api/ops')>()),
  callOp: (name: string, input: unknown, options: unknown) => callOp(name, input, options),
}))
vi.mock('../../collections/api', () => ({
  useCollections: () => ({ isPending: false, data: { collections: [{ id: 'c1', name: 'Bugs' }], canBuild: true } }),
}))
vi.mock('../../experience/api', () => ({ refreshLayouts: () => Promise.resolve() }))

const studio = {
  id: 's1',
  organizationId: 'w1',
  key: 'STU',
  name: 'Studio',
  archivedAt: null,
  role: 'owner',
  version: 1,
  attentionCount: 0,
} as unknown as Project

afterEach(() => {
  cleanup()
  callOp.mockReset()
})

describe('BuiltModules', () => {
  it('builds a module from a space’s parts, with the ones chosen for the sidebar', async () => {
    callOp.mockImplementation(async (name: string) => {
      switch (name) {
        case 'module_list':
          return { built: [], modules: [], workspaceId: 'w1' }
        case 'page_list':
          return {
            pages: [{ id: 'p1', title: 'Triage guide', excerpt: '', isGuide: false, stale: false, updatedAt: '2026-11-19T00:00:00Z', version: 1 }],
          }
        case 'module_create':
          return {
            id: 'm1',
            key: 'built.bug_triage',
            name: 'Bug triage',
            description: '',
            archived: false,
            nav: [],
            on: true,
            parts: [],
            revision: 1,
          }
        default:
          return null
      }
    })
    renderPiece(<BuiltModules canBuild projects={[studio]} workspaceId="w1" />)
    fireEvent.click(await screen.findByRole('button', { name: 'Build a module' }))

    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Bug triage' } })
    expect(screen.getByLabelText(/^Key/)).toHaveValue('bug_triage')
    fireEvent.click(await screen.findByRole('checkbox', { name: /Bugs/ }))
    fireEvent.click(screen.getByRole('checkbox', { name: /Triage guide/ }))
    // A part joins the sidebar when it joins the module; this page stays out.
    const sidebar = screen.getAllByRole('checkbox', { name: 'In the sidebar' })
    expect(sidebar).toHaveLength(2)
    fireEvent.click(sidebar[1])
    fireEvent.click(screen.getByRole('button', { name: 'Build' }))

    await waitFor(() =>
      expect(callOp).toHaveBeenCalledWith(
        'module_create',
        {
          key: 'bug_triage',
          name: 'Bug triage',
          description: null,
          icon: 'table',
          spaceId: 's1',
          parts: [
            { kind: 'collection', id: 'c1' },
            { kind: 'page', id: 'p1' },
          ],
          nav: [{ label: 'Bugs', icon: 'table', target: { kind: 'collection', collection: 'c1' } }],
        },
        expect.objectContaining({ workspaceId: 'w1' }),
      ),
    )
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
  })
})
