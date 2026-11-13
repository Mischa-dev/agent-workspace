import { describe, expect, it } from 'vitest'

import { thingAt } from './thingLinks'

const origin = 'https://work.example'
const project = '01a0079d-f61a-7431-b384-72fb3265ce4f'
const id = '01A180E0-2C93-7A32-9D9B-BBCF792CE2ED'

describe('thingAt', () => {
  it('reads the addresses the server keeps as references', () => {
    expect(thingAt(`/projects/${project}/work-items/${id}`, origin)).toEqual({
      kind: 'task',
      id: id.toLowerCase(),
      path: `/projects/${project}/work-items/${id}`,
    })
    expect(thingAt(`${origin}/projects/${project}/pages/${id}#intro`, origin)).toMatchObject({ kind: 'page', path: `/projects/${project}/pages/${id}#intro` })
    expect(thingAt(`/projects/${project}/threads/${id}`, origin)?.kind).toBe('thread')
    expect(thingAt(`/projects/${project}/channels/${id}`, origin)?.kind).toBe('channel')
    expect(thingAt(`/whiteboards/${id}`, origin)?.kind).toBe('whiteboard')
    expect(thingAt(`/messages/${id}/`, origin)?.kind).toBe('conversation')
    expect(thingAt(`/messages/${project}?thread=${id}`, origin)).toMatchObject({ kind: 'thread', id: id.toLowerCase() })
  })

  it('leaves every other address alone', () => {
    expect(thingAt(`https://elsewhere.example/whiteboards/${id}`, origin)).toBeNull()
    expect(thingAt(`/projects/${project}/work-items`, origin)).toBeNull()
    expect(thingAt(`/projects/${project}/work-items/${id}/edit`, origin)).toBeNull()
    expect(thingAt('/whiteboards/not-an-id', origin)).toBeNull()
    expect(thingAt('mailto:someone@example.com', origin)).toBeNull()
  })
})
