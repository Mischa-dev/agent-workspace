import { describe, expect, it } from 'vitest'

import { thingAt, thingsLinkedIn } from './thingLinks'

const origin = 'https://work.example'
const project = '01a0079d-f61a-7431-b384-72fb3265ce4f'
const id = '01A180E0-2C93-7A32-9D9B-BBCF792CE2ED'
const other = '01a0079d-f61a-7431-b384-72fb3265ce4e'

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

describe('thingAt and the manifests', () => {
  const manifests = import.meta.glob<{ kind: string; href: string }[]>(['../../../../../modules/*.json', '!../../../../../modules/presets.json'], {
    eager: true,
    import: 'things',
  })

  it('reads every address a manifest gives a thing of its own', () => {
    const declared = Object.values(manifests).flatMap((things) => things ?? [])
    expect(declared.length).toBeGreaterThan(5)
    for (const thing of declared) {
      // An address with no id of its own (a list, a settings page, an outside url) isn't a link to one thing.
      if (!/\{id\}/.test(thing.href) || thing.href.startsWith('{')) continue
      const href = thing.href.replace('{id}', id).replace(/\{[a-zA-Z]+\}/g, other)
      expect(thingAt(href, origin), `${thing.kind}: ${thing.href}`).toMatchObject({ kind: thing.kind, id: id.toLowerCase() })
    }
  })

  it('reads the workspace addresses too', () => {
    expect(thingAt(`/w/acme/dm/${id}`, origin)?.kind).toBe('conversation')
    expect(thingAt(`/w/acme/whiteboards/${id}`, origin)?.kind).toBe('whiteboard')
    expect(thingAt(`/w/acme/s/${project}/tasks/${id}`, origin)?.kind).toBe('task')
    expect(thingAt(`/w/acme/s/${project}/chat/${id}`, origin)?.kind).toBe('channel')
    expect(thingAt(`/w/acme/s/${project}`, origin)).toMatchObject({ kind: 'project', id: project })
  })
})

describe('thingsLinkedIn', () => {
  it('finds each linked thing once, in order, up to the limit', () => {
    const text = [
      `See ${origin}/projects/${project}/work-items/${id}.`,
      `and [the page](/projects/${project}/pages/${project}) and again ${origin}/projects/${project}/work-items/${id}`,
      `plus /whiteboards/${other} and https://elsewhere.example/whiteboards/${id}`,
    ].join(' ')
    expect(thingsLinkedIn(text, 3, origin).map((address) => address.kind)).toEqual(['task', 'page', 'whiteboard'])
    expect(thingsLinkedIn(text, 1, origin)).toHaveLength(1)
    expect(thingsLinkedIn('nothing here', 3, origin)).toEqual([])
  })
})
