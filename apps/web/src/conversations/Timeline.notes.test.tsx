import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it } from 'vitest'

import { TimelineEntries } from './Timeline'
import type { ConversationActor, ConversationTimelineEntry } from './types'

afterEach(cleanup)

const person = (id: string, displayName: string, kind: ConversationActor['kind'] = 'external_agent'): ConversationActor => ({
  id,
  kind,
  displayName,
  handle: displayName.toLowerCase(),
  avatarUrl: null,
})

const mara = person('a-mara', 'Mara')
const rook = person('a-rook', 'Rook')
const dana = person('p-dana', 'Dana', 'human')

function entry(
  sequence: number,
  author: ConversationActor,
  text: string,
  extra: { note?: boolean; mentions?: ConversationActor[] } = {},
): ConversationTimelineEntry {
  const id = `m-${sequence}`
  return {
    id: `e-${sequence}`,
    sequence,
    kind: 'message',
    createdAt: new Date(Date.UTC(2026, 10, 27, 20, sequence)).toISOString(),
    workerEvent: null,
    message: {
      id,
      author,
      content: { version: 1, nodes: [{ type: 'text', text }] },
      plainText: text,
      mentionedActors: extra.mentions ?? [],
      referencedWorkItems: [],
      createdAt: new Date(Date.UTC(2026, 10, 27, 20, sequence)).toISOString(),
      via: null,
      revision: 1,
      editedAt: null,
      removed: null,
      note: extra.note,
    },
  }
}

function view(entries: ConversationTimelineEntry[], props: { foldNotes?: boolean; at?: string } = {}) {
  return render(
    <MemoryRouter initialEntries={[props.at ?? '/']}>
      <TimelineEntries entries={entries} foldNotes={props.foldNotes} viewerId={dana.id} />
    </MemoryRouter>,
  )
}

describe('working notes in a timeline', () => {
  const entries = [
    entry(1, dana, 'Please split the importer.'),
    entry(2, mara, 'I have parse.rs.', { note: true }),
    entry(3, rook, 'Then I take write.rs.', { note: true }),
    entry(4, mara, 'Both halves are up.'),
  ]

  it('folds a run of notes to one line and opens it on request', () => {
    view(entries)
    expect(screen.getByText('Please split the importer.')).toBeTruthy()
    expect(screen.getByText('Both halves are up.')).toBeTruthy()
    expect(document.querySelectorAll('.aw-msg')).toHaveLength(2)
    const fold = screen.getByRole('button', { name: /2 working notes/ })
    expect(fold.textContent).toContain('Mara and Rook')
    // The newest note's first line is the preview.
    expect(fold.textContent).toContain('Then I take write.rs.')
    fireEvent.click(fold)
    expect(screen.getByText('I have parse.rs.')).toBeTruthy()
    expect(fold.getAttribute('aria-expanded')).toBe('true')
  })

  it('never folds a note that mentions you or one you wrote', () => {
    view([
      entry(1, mara, 'Quiet one.', { note: true }),
      entry(2, mara, 'Dana, we need a folder name.', { note: true, mentions: [dana] }),
      entry(3, dana, 'My own note.', { note: true }),
    ])
    expect(document.querySelectorAll('.aw-msg')).toHaveLength(2)
    expect(screen.getByText('Dana, we need a folder name.')).toBeTruthy()
    expect(screen.getByText('My own note.')).toBeTruthy()
    expect(screen.getByRole('button', { name: /1 working note/ })).toBeTruthy()
  })

  it('shows notes as written in a working channel', () => {
    view(entries, { foldNotes: false })
    expect(screen.getByText('I have parse.rs.')).toBeTruthy()
    expect(screen.queryByRole('button', { name: /working note/ })).toBeNull()
  })

  it('opens the fold a linked note is in', () => {
    view(entries, { at: '/#message-m-3' })
    expect(screen.getByText('Then I take write.rs.')).toBeTruthy()
  })
})
