import { render, screen } from '@testing-library/react'
import axe from 'axe-core'
import { MemoryRouter } from 'react-router-dom'
import { describe, expect, it } from 'vitest'

import { DECLARED_EMBEDS, DECLARED_THINGS, EVERY_PIECE } from './declared'
import { SamplePiece } from './PieceGallery'
import { PieceView } from './Pieces'

// Every piece a manifest declares draws with made-up things and passes axe.
// Widths and colour schemes are checked in a real browser (e2e/pieces.spec.ts);
// jsdom can't lay pages out.

const run = async (container: HTMLElement) =>
  (await axe.run(container, { rules: { 'color-contrast': { enabled: false } } })).violations

describe('pieces the app draws', () => {
  it('knows the kinds of thing modules declare', () => {
    const kinds = DECLARED_THINGS.map((entry) => entry.decl.kind)
    for (const kind of ['task', 'page', 'whiteboard', 'thread', 'run']) expect(kinds).toContain(kind)
    expect(DECLARED_EMBEDS.length).toBeGreaterThan(5)
  })

  it.each(DECLARED_EMBEDS.map(({ embed }) => [embed.id, embed] as const))('draws %s', async (_, embed) => {
    const { container } = render(
      <MemoryRouter>
        <SamplePiece label={embed.label} piece={embed.piece} />
      </MemoryRouter>,
    )
    expect(container.querySelector(`[data-piece="${embed.piece.type}"]`)).not.toBeNull()
    expect(await run(container)).toEqual([])
  })

  it.each(EVERY_PIECE)('draws a %s of tasks', async (type) => {
    const { container } = render(
      <MemoryRouter>
        <SamplePiece label={`Sample ${type}`} piece={{ type, thing: 'task', fields: ['status', 'owner', 'priority', 'dueDate'], groupBy: 'status', actions: ['open', 'edit'] }} />
      </MemoryRouter>,
    )
    expect(container.querySelector(`[data-piece="${type}"]`)).not.toBeNull()
    expect(await run(container)).toEqual([])
  })

  it('a table says what each cell is, for its narrow form', () => {
    render(
      <MemoryRouter>
        <SamplePiece label="Tasks" piece={{ type: 'table', thing: 'task', fields: ['status', 'dueDate'] }} />
      </MemoryRouter>,
    )
    const table = screen.getByRole('table', { name: 'Tasks' })
    for (const cell of table.querySelectorAll('td')) expect(cell.getAttribute('data-label')).toBeTruthy()
  })

  it('a list shows labels as chips in their tones, a moment as how long ago, and long text under the title', async () => {
    const page = DECLARED_THINGS.find((entry) => entry.decl.kind === 'page')!.decl
    const { container } = render(
      <MemoryRouter>
        <PieceView
          decl={page}
          label="Pages"
          piece={{ type: 'list', thing: 'page', fields: ['tags', 'updatedAt', 'owner', 'excerpt'] }}
          things={[
            {
              id: 'p1',
              kind: 'page',
              module: 'pages',
              title: 'Guide',
              href: '/projects/x/pages/p1',
              owner: { id: 'a', name: 'Ana' },
              fields: { tags: ['Guide', 'Not updated in months'], updatedAt: new Date().toISOString(), excerpt: 'How we work here.' },
            },
            { id: 'p2', kind: 'page', module: 'pages', title: 'Notes', href: '/projects/x/pages/p2', fields: { tags: [], updatedAt: null, excerpt: '' } },
          ]}
        />
      </MemoryRouter>,
    )
    const [first, second] = container.querySelectorAll('.aw-piece__row')
    expect(first.querySelector('.aw-chip--agent')?.textContent).toBe('Guide')
    expect(first.querySelector('.aw-chip--warning')?.textContent).toBe('Not updated in months')
    expect(first.textContent).toContain('Edited')
    expect(first.textContent).toContain('by Ana')
    expect(first.querySelector('time')?.textContent).toBe('just now')
    expect(first.querySelector('.aw-piece__summary')?.textContent).toBe('How we work here.')
    // No tags and no summary leave nothing behind.
    expect(second.querySelector('.aw-piece__chips')).toBeNull()
    expect(second.querySelector('.aw-piece__summary')).toBeNull()
    expect(await run(container)).toEqual([])
  })
})
