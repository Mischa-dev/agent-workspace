import { describe, expect, it } from 'vitest'

import { moduleEmbedTarget } from './moduleEmbed'

describe('module embeds', () => {
  it('shows one thing with the card its module declares', () => {
    const task = moduleEmbedTarget('/projects/p1/work-items/t1')
    expect(task).toMatchObject({ kind: 'thing', thing: 'task', id: 't1', embed: { id: 'tasks.card' } })
    expect(moduleEmbedTarget('/whiteboards/w1')).toMatchObject({ thing: 'whiteboard', id: 'w1', embed: { id: 'whiteboards.card' } })
    expect(moduleEmbedTarget('/projects/p1/pages/x')).toMatchObject({ thing: 'page', embed: { id: 'pages.card' } })
    expect(moduleEmbedTarget('/projects/p1/threads/c1')).toMatchObject({ thing: 'thread', embed: { id: 'channels.thread_card' } })
  })

  it('shows a project’s tasks as a list, or a board, with only known filters', () => {
    const list = moduleEmbedTarget('/projects/p1/work-items?semanticCategory=started&priority=high&sort=oldest&secret=x&task=t9')
    expect(list).toMatchObject({
      kind: 'tasks',
      projectId: 'p1',
      filters: { semanticCategory: 'started', priority: 'high', sort: 'oldest' },
      embed: { id: 'tasks.list' },
    })
    expect(list?.kind === 'tasks' && Object.keys(list.filters).sort()).toEqual(['priority', 'semanticCategory', 'sort'])
    const board = moduleEmbedTarget('/projects/p1/work-items?view=board&priority=high')
    expect(board).toMatchObject({ embed: { id: 'tasks.board' }, filters: { priority: 'high' } })
    expect(board?.kind === 'tasks' && 'view' in board.filters).toBe(false)
    expect(moduleEmbedTarget('/projects/p1/work-items?layout=board')).toMatchObject({ embed: { id: 'tasks.board' } })
  })

  it('leaves other addresses as links', () => {
    expect(moduleEmbedTarget('https://example.com/projects/p1/work-items/t1')).toBeNull()
    expect(moduleEmbedTarget('/projects/p1/collections/c1')).toBeNull()
    expect(moduleEmbedTarget('/runs/r1')).toBeNull()
    expect(moduleEmbedTarget('/projects/p1/work-items/t1/extra')).toBeNull()
  })
})

describe('pasted addresses', () => {
  it('keeps the path of an address in this app and nothing else', async () => {
    const { embedPath } = await import('./moduleEmbed')
    expect(embedPath('https://app.example/projects/p1/work-items/t1', 'https://app.example')).toBe('/projects/p1/work-items/t1')
    expect(embedPath(' /projects/p1/work-items?status=s ', 'https://app.example')).toBe('/projects/p1/work-items?status=s')
    expect(embedPath('https://elsewhere.example/projects/p1', 'https://app.example')).toBeNull()
    expect(embedPath('', 'https://app.example')).toBeNull()
  })
})
