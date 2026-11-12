import { Editor } from '@tiptap/core'
import { TextSelection } from '@tiptap/pm/state'
import { afterEach, describe, expect, it } from 'vitest'

import { DiscussionMarks, discussionKey, findQuote, selectionAnchor } from './highlights'
import { pageExtensions } from './schema'

// Comments are anchored to a block's id and the words quoted from it; the
// server checks quotes the same way (apps/server/src/page_docs/comments.rs),
// so a comment the server takes is one the editor can mark.

let editor: Editor | null = null

afterEach(() => {
  editor?.destroy()
  editor = null
})

function open() {
  editor = new Editor({
    extensions: [...pageExtensions(), DiscussionMarks],
    content: {
      type: 'doc',
      content: [
        { type: 'heading', attrs: { level: 1, id: 'head0001' }, content: [{ type: 'text', text: 'Plan' }] },
        {
          type: 'paragraph',
          attrs: { id: 'para0001' },
          content: [
            { type: 'text', text: 'Ship the ' },
            { type: 'text', text: 'first', marks: [{ type: 'bold' }] },
            { type: 'text', text: ' slice  this week.' },
          ],
        },
        {
          type: 'bulletList',
          attrs: { id: 'list0001' },
          content: [
            { type: 'listItem', content: [{ type: 'paragraph', content: [{ type: 'text', text: 'one' }] }] },
            { type: 'listItem', content: [{ type: 'paragraph', content: [{ type: 'text', text: 'two' }] }] },
          ],
        },
        { type: 'paragraph', content: [{ type: 'text', text: 'Unsaved' }] },
        {
          type: 'paragraph',
          attrs: { id: 'para0002' },
          content: [{ type: 'text', text: 'first' }, { type: 'hardBreak' }, { type: 'text', text: 'second' }],
        },
      ],
    },
  })
  return editor
}

function block(instance: Editor, index: number) {
  let found: { node: Parameters<typeof findQuote>[0]; offset: number } | null = null
  instance.state.doc.forEach((node, offset, at) => {
    if (at === index) found = { node, offset }
  })
  if (!found) throw new Error(`no block ${index}`)
  return found as { node: Parameters<typeof findQuote>[0]; offset: number }
}

describe('comment anchors', () => {
  it('finds quoted words across marks and runs of spaces', () => {
    const instance = open()
    const { node, offset } = block(instance, 1)
    const found = findQuote(node, offset, 'first slice this')
    expect(found).not.toBeNull()
    expect(instance.state.doc.textBetween(found!.from, found!.to)).toBe('first slice  this')
    expect(findQuote(node, offset, 'second')).toBeNull()
  })

  it('finds words across a list’s items, as the server does', () => {
    const instance = open()
    const { node, offset } = block(instance, 2)
    const found = findQuote(node, offset, 'one two')
    expect(found).not.toBeNull()
    expect(instance.state.doc.textBetween(found!.from, found!.to, '\n')).toBe('one\ntwo')
  })

  it('makes an anchor from the selection, within one block', () => {
    const instance = open()
    const { offset } = block(instance, 1)
    // From "the" into the list: the quote stops at the paragraph's end.
    instance.view.dispatch(instance.state.tr.setSelection(TextSelection.create(instance.state.doc, offset + 6, offset + 40)))
    expect(selectionAnchor(instance.state)).toEqual({ blockId: 'para0001', quote: 'the first slice  this week.' })
    instance.view.dispatch(instance.state.tr.setSelection(TextSelection.create(instance.state.doc, offset + 2)))
    expect(selectionAnchor(instance.state)).toBe('empty')
  })

  it('says when the block has no id yet', () => {
    const instance = open()
    const { offset, node } = block(instance, 3)
    instance.view.dispatch(instance.state.tr.setSelection(TextSelection.create(instance.state.doc, offset + 1, offset + node.nodeSize - 1)))
    expect(selectionAnchor(instance.state)).toBe('unsaved')
  })

  it('reads a line break inside a paragraph as a break, as the server does', () => {
    const instance = open()
    const { node, offset } = block(instance, 4)
    instance.view.dispatch(instance.state.tr.setSelection(TextSelection.create(instance.state.doc, offset + 1, offset + node.nodeSize - 1)))
    expect(selectionAnchor(instance.state)).toEqual({ blockId: 'para0002', quote: 'first\nsecond' })
    const found = findQuote(node, offset, 'first second')
    expect(found).not.toBeNull()
    expect(findQuote(node, offset, 'firstsecond')).toBeNull()
  })

  it('marks open threads and suggested blocks without changing the page', () => {
    const instance = open()
    const before = instance.getJSON()
    instance.view.dispatch(
      instance.state.tr.setMeta(discussionKey, {
        threads: [{ id: 't1', blockId: 'para0001', quote: 'first slice' }],
        active: 't1',
        suggested: ['list0001'],
      }),
    )
    expect(instance.getJSON()).toEqual(before)
    const marked = instance.view.dom.querySelectorAll('[data-thread="t1"]')
    expect(marked.length).toBeGreaterThan(0)
    expect(marked[0].classList.contains('is-active')).toBe(true)
    expect(instance.view.dom.querySelector('.aw-live__suggested')).not.toBeNull()
  })
})
