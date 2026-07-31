import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { MessageContent } from './MessageContent'
import { normalizeTimelinePage } from './api'
import { parseMessageContent } from './messageContentModel'

const actor = {
  id: '019fb8b0-0000-7000-8000-000000000001',
  kind: 'human' as const,
  displayName: 'Mischa <Owner>',
  handle: 'mischa',
  avatarUrl: null,
}

describe('MessageContent', () => {
  it('renders hostile text as text and structured mentions semantically', () => {
    const { container } = render(
      <MessageContent
        actors={[actor]}
        content={{
          version: 1,
          nodes: [
            { type: 'text', text: '<img src=x onerror=alert(1)> Hello ' },
            { type: 'mention', actorId: actor.id },
          ],
        }}
        fallbackPlainText="fallback"
      />,
    )

    expect(
      screen.getByText('<img src=x onerror=alert(1)> Hello', { exact: true }),
    ).toBeInTheDocument()
    expect(container.querySelector('img')).toBeNull()
    expect(screen.getByLabelText('Mischa <Owner>, @mischa')).toHaveTextContent(
      '@mischa',
    )
  })

  it('uses an escaped plain-text fallback for malformed or future content', () => {
    const { container } = render(
      <MessageContent
        actors={[]}
        content={{ version: 2, nodes: [{ type: 'html', html: '<b>bad</b>' }] }}
        fallbackPlainText="<b>safe fallback</b>"
      />,
    )

    expect(screen.getByText('<b>safe fallback</b>')).toBeInTheDocument()
    expect(container.querySelector('b')).toBeNull()
  })

  it('renders a safe unavailable marker when resolved Actor data is absent', () => {
    render(
      <MessageContent
        actors={[]}
        content={{
          version: 1,
          nodes: [{ type: 'mention', actorId: actor.id }],
        }}
        fallbackPlainText="@former-member"
      />,
    )

    expect(screen.getByLabelText('Unavailable Actor mention')).toHaveTextContent(
      '@unavailable',
    )
  })
})

describe('parseMessageContent', () => {
  it('rejects unknown fields, invalid identifiers, and configured bounds', () => {
    expect(
      parseMessageContent({
        version: 1,
        nodes: [{ type: 'mention', actorId: 'not-a-uuid' }],
      }),
    ).toBeNull()
    expect(
      parseMessageContent({
        version: 1,
        nodes: [{ type: 'text', text: 'hello', html: '<b>hello</b>' }],
      }),
    ).toBeNull()
    expect(
      parseMessageContent({
        version: 1,
        nodes: Array.from({ length: 129 }, () => ({ type: 'text', text: 'x' })),
      }),
    ).toBeNull()
  })

  it('counts Unicode scalar values and rejects unsupported controls', () => {
    const exact = '🙂'.repeat(20_000)
    expect(
      parseMessageContent({
        version: 1,
        nodes: [{ type: 'text', text: exact }],
      }),
    ).not.toBeNull()
    expect(
      parseMessageContent({
        version: 1,
        nodes: [{ type: 'text', text: `${exact}🙂` }],
      }),
    ).toBeNull()
    expect(
      parseMessageContent({
        version: 1,
        nodes: [{ type: 'text', text: 'safe\n\t\rtext' }],
      }),
    ).not.toBeNull()
    expect(
      parseMessageContent({
        version: 1,
        nodes: [{ type: 'text', text: 'unsafe\u0000text' }],
      }),
    ).toBeNull()
    expect(
      parseMessageContent({
        version: 1,
        nodes: [{ type: 'text', text: 'unsafe\u0085text' }],
      }),
    ).toBeNull()
    expect(
      parseMessageContent({
        version: 1,
        nodes: [{ type: 'text', text: 'unsafe\ud800text' }],
      }),
    ).toBeNull()
  })

  it('keeps invalid persisted Messages outside the shared timeline projection', () => {
    const timeline = normalizeTimelinePage({
      conversation: {
        id: 'conversation-1',
        projectId: 'project-1',
        kind: 'thread',
        name: 'Boundary test',
      },
      entries: [
        {
          id: 'entry-1',
          sequence: 1,
          kind: 'message',
          createdAt: '2026-07-31T16:00:00Z',
          message: {
            id: 'message-1',
            author: actor,
            content: {
              version: 1,
              nodes: [{ type: 'text', text: 'unsafe\u0000content' }],
            },
            plainText: 'unsafe\u0000content',
          },
        },
      ],
    })

    expect(timeline.entries).toEqual([])
  })
})
