import { describe, expect, it } from 'vitest'

import { messageFromShare, readSharedContent, taskFromShare } from './sharedContent'

const share = (values: Record<string, string>) => readSharedContent(new URLSearchParams(values))

describe('shared content', () => {
  it('finds a link that an app put in the text', () => {
    expect(share({ text: 'Look at this https://example.com/a?b=1.' })).toEqual({
      title: '',
      text: 'Look at this',
      url: 'https://example.com/a?b=1',
    })
  })

  it('makes a task from a title, or the first line, with the link in the description', () => {
    const titled = share({ title: 'Flaky deploy', text: 'Fails on the second try', url: 'https://ci.example/run/1' })!
    expect(taskFromShare(titled)).toEqual({ title: 'Flaky deploy', description: 'Fails on the second try\n\nhttps://ci.example/run/1' })
    const untitled = share({ text: 'Fix the login page\nIt loops on Safari' })!
    expect(taskFromShare(untitled)).toEqual({ title: 'Fix the login page', description: 'It loops on Safari' })
    expect(taskFromShare(share({ url: 'https://github.com/x/y/issues/4' })!).title).toBe('github.com')
  })

  it('posts everything as a message and ignores empty or non-web shares', () => {
    expect(messageFromShare(share({ title: 'Read', url: 'https://example.com' })!)).toBe('Read\nhttps://example.com')
    expect(share({})).toBeNull()
    expect(share({ url: 'javascript:alert(1)' })).toBeNull()
  })
})
