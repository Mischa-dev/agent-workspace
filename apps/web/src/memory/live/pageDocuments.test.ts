/// <reference types="node" />
import { readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

import { Editor } from '@tiptap/core'
import { Collaboration } from '@tiptap/extension-collaboration'
import { afterEach, beforeAll, describe, expect, it } from 'vitest'
import * as Y from 'yjs'

import { insertEmbed, insertTable, setLink, toggleTasks } from './commands'
import documents from './pageDocuments.json'
import { pageExtensions, repeatedIds } from './schema'

// The other half of apps/server/src/page_docs/parity.rs. The server makes
// each document in pageDocuments.json from Markdown; the editor must open it
// without changing a thing. The edits below are made the way people make
// them; pageEdits.json keeps the updates they send, and the server checks it
// takes each one and writes the Markdown given here. After a change to the
// editor, rewrite pageEdits.json with UPDATE_DOC_PARITY=1.

// Vitest runs from apps/web; import.meta.url isn't a file URL under jsdom.
const EDITS = join(process.cwd(), 'src/memory/live/pageEdits.json')
const ABOUT =
  'Edits the page editor makes to documents in pageDocuments.json, as the updates it sends (apps/web/src/memory/live/pageDocuments.test.ts writes them, from a fixed client id). The server checks it takes each one and writes the Markdown given. Rewrite with UPDATE_DOC_PARITY=1.'

type Sample = { name: string; markdown: string; state: string }

const samples = documents.documents as Sample[]

function bytes(base64: string) {
  return Uint8Array.from(atob(base64), (char) => char.charCodeAt(0))
}

function base64(data: Uint8Array) {
  let binary = ''
  for (const byte of data) binary += String.fromCharCode(byte)
  return btoa(binary)
}

let editor: Editor | null = null

afterEach(() => {
  editor?.destroy()
  editor = null
})

beforeAll(() => {
  // jsdom has no layout, and ProseMirror measures when it scrolls the cursor into view.
  const empty = () => ({ x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0, toJSON: () => ({}) }) as DOMRect
  Range.prototype.getBoundingClientRect ??= empty
  Range.prototype.getClientRects ??= () => ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] }) as unknown as DOMRectList
  document.elementFromPoint ??= () => null
})

/** An editor on a sample document, as a page opens it; `updates` collects what it would send. */
function open(name: string) {
  const sample = samples.find((candidate) => candidate.name === name)
  if (!sample) throw new Error(`no sample ${name}`)
  const doc = new Y.Doc()
  doc.clientID = 4242
  Y.applyUpdate(doc, bytes(sample.state), 'server')
  const server = Y.encodeStateVector(doc)
  const updates: Uint8Array[] = []
  doc.on('update', (update: Uint8Array, origin: unknown) => {
    if (origin !== 'server') updates.push(update)
  })
  const errors: Error[] = []
  editor = new Editor({
    element: document.createElement('div'),
    extensions: [...pageExtensions(), Collaboration.configure({ document: doc })],
    enableContentCheck: true,
    injectCSS: false,
    onContentError: ({ error }) => errors.push(error),
  })
  return { editor, doc, server, updates, errors }
}

/** Where `text` first appears in the editor's document. */
function find(target: Editor, text: string, occurrence = 0) {
  let found: { from: number; to: number } | null = null
  let seen = 0
  target.state.doc.descendants((node, position) => {
    if (found || !node.isText || !node.text) return !found
    let index = node.text.indexOf(text)
    while (index >= 0) {
      if (seen === occurrence) {
        found = { from: position + index, to: position + index + text.length }
        return false
      }
      seen += 1
      index = node.text.indexOf(text, index + 1)
    }
    return true
  })
  if (!found) throw new Error(`no ${text} in the document`)
  return found as { from: number; to: number }
}

function type(target: Editor, text: string) {
  target.view.dispatch(target.state.tr.insertText(text))
}

/** Types the way a keyboard does, a key at a time, so Markdown shortcuts like "- " run. */
function typeKeys(target: Editor, text: string) {
  for (const char of text) {
    const { from, to } = target.state.selection
    const insert = () => target.state.tr.insertText(char, from, to)
    const handled = target.view.someProp('handleTextInput', (handle) => handle(target.view, from, to, char, insert))
    if (!handled) target.view.dispatch(insert())
  }
}

describe('documents the server makes', () => {
  it.each(samples.map((sample) => [sample.name, sample] as const))('%s opens without a change', (name) => {
    const { editor: opened, updates, errors } = open(name)
    expect(errors).toEqual([])
    expect(opened.isEditable).toBe(true)
    expect(() => opened.state.doc.check()).not.toThrow()
    expect(updates).toEqual([])
  })

  it('shows every sample', () => {
    expect(samples.map((sample) => sample.name)).toEqual(
      expect.arrayContaining(['empty', 'marks', 'lists', 'code', 'table', 'quote and rule', 'blocks in items', 'embeds']),
    )
  })
})

type EditCase = { name: string; document: string; markdown: string; edit: (target: Editor) => void }

// What people do in the editor, and the Markdown the server writes for it.
const EDIT_CASES: EditCase[] = [
  {
    name: 'type into an empty page',
    document: 'empty',
    markdown: 'Hello there',
    edit: (target) => {
      target.commands.setTextSelection(1)
      type(target, 'Hello there')
    },
  },
  {
    name: 'type inside a paragraph',
    document: 'paragraphs',
    markdown: 'One line, edited\nand the next.\n\nA second paragraph with a hard  \nbreak.\n',
    edit: (target) => {
      target.commands.setTextSelection(find(target, 'One line').to)
      type(target, ', edited')
    },
  },
  {
    name: 'make a word bold',
    document: 'paragraphs',
    // A changed block is written afresh: its line break comes out plain.
    markdown: 'One line\nand the next.\n\nA **second** paragraph with a hard\nbreak.\n',
    edit: (target) => {
      target.chain().setTextSelection(find(target, 'second')).toggleBold().run()
    },
  },
  {
    name: 'link a word',
    document: 'paragraphs',
    markdown: 'One line\nand the next.\n\nA second [paragraph](https://example.org/p) with a hard\nbreak.\n',
    edit: (target) => {
      target.chain().setTextSelection(find(target, 'paragraph')).setMark('link', { href: 'https://example.org/p' }).run()
    },
  },
  {
    name: 'split a paragraph',
    document: 'paragraphs',
    markdown: 'One line\nand the next.\n\nA second\n\nparagraph with a hard\nbreak.\n',
    edit: (target) => {
      target.commands.setTextSelection(find(target, ' paragraph').from)
      target.commands.keyboardShortcut('Enter')
      target.commands.deleteRange({ from: target.state.selection.from, to: target.state.selection.from + 1 })
    },
  },
  {
    name: 'a new paragraph at the end',
    document: 'paragraphs',
    markdown: 'One line\nand the next.\n\nA second paragraph with a hard  \nbreak.\n\nThird.\n',
    edit: (target) => {
      target.commands.setTextSelection(find(target, 'break.').to)
      target.commands.keyboardShortcut('Enter')
      type(target, 'Third.')
    },
  },
  {
    name: 'Enter at the start of a paragraph',
    document: 'paragraphs',
    markdown: 'One line\nand the next.\n\nA second paragraph with a hard  \nbreak.\n',
    edit: (target) => {
      target.commands.setTextSelection(find(target, 'A second').from)
      target.commands.keyboardShortcut('Enter')
    },
  },
  {
    name: 'make a heading',
    document: 'paragraphs',
    // Markdown headings are one line.
    markdown: '## One line and the next.\n\nA second paragraph with a hard  \nbreak.\n',
    edit: (target) => {
      target.chain().setTextSelection(find(target, 'One').from).toggleHeading({ level: 2 }).run()
    },
  },
  {
    name: 'make a list',
    document: 'headings',
    markdown: '# One\n\n- Two\n\n### Three\n\n#### Four\n\n##### Five\n\n###### Six\n',
    edit: (target) => {
      target.chain().setTextSelection(find(target, 'Two').from).setParagraph().toggleBulletList().run()
    },
  },
  {
    name: 'remove a heading',
    document: 'headings',
    markdown: '# One\n\n### Three\n\n#### Four\n\n##### Five\n\n###### Six\n',
    edit: (target) => {
      const { from, to } = find(target, 'Two')
      target.commands.deleteRange({ from: from - 1, to: to + 1 })
    },
  },
  {
    name: 'tick a task',
    document: 'lists',
    markdown: '- one\n- two\n  - nested\n\n3. three\n4. four\n\n- [x] to do\n- [x] done\n',
    edit: (target) => {
      const { from } = find(target, 'to do')
      const item = target.state.doc.resolve(from).before(-1)
      target.view.dispatch(target.state.tr.setNodeAttribute(item, 'checked', true))
    },
  },
  {
    name: 'a new task after a done one',
    document: 'lists',
    markdown: '- one\n- two\n  - nested\n\n3. three\n4. four\n\n- [ ] to do\n- [x] done\n- [ ] next\n',
    edit: (target) => {
      target.commands.setTextSelection(find(target, 'done').to)
      target.commands.keyboardShortcut('Enter')
      type(target, 'next')
    },
  },
  {
    name: 'indent an item',
    document: 'lists',
    markdown: '- one\n- two\n  - nested\n\n3. three\n   1. four\n\n- [ ] to do\n- [x] done\n',
    edit: (target) => {
      target.commands.setTextSelection(find(target, 'four').from)
      target.commands.keyboardShortcut('Tab')
    },
  },
  {
    name: 'a code block',
    document: 'empty',
    markdown: '```\nlet x = 1\n```',
    edit: (target) => {
      target.chain().setTextSelection(1).toggleCodeBlock().run()
      type(target, 'let x = 1')
    },
  },
  {
    name: 'type in code',
    document: 'code',
    markdown: '```rust\nfn main() { run() }\n```\n\n```\nplain\n```\n',
    edit: (target) => {
      target.commands.setTextSelection(find(target, '{}').from + 1)
      type(target, ' run() ')
    },
  },
  {
    name: 'a table',
    document: 'empty',
    markdown: '| a |  |\n| --- | --- |\n|  |  |',
    edit: (target) => {
      target.chain().setTextSelection(1).insertTable({ rows: 2, cols: 2, withHeaderRow: true }).run()
      type(target, 'a')
    },
  },
  {
    name: 'type in a cell',
    document: 'table',
    markdown: '| a | b | c |\n| :-- | :-: | --: |\n| 1 | **2** | `3` |\n| 4 | five | 6 |\n',
    edit: (target) => {
      // keyboardShortcut() keeps a shortcut's steps but not where it moves the cursor.
      target.chain().setTextSelection(find(target, '4').to).goToNextCell().run()
      type(target, 'five')
    },
  },
  {
    name: 'add a row',
    document: 'table',
    markdown: '| a | b | c |\n| :-- | :-: | --: |\n| 1 | **2** | `3` |\n| 4 |  | 6 |\n|  |  |  |\n',
    edit: (target) => {
      target.chain().setTextSelection(find(target, '6').from).addRowAfter().run()
    },
  },
  {
    name: 'quote a paragraph',
    document: 'paragraphs',
    markdown: '> One line\n> and the next.\n\nA second paragraph with a hard  \nbreak.\n',
    edit: (target) => {
      target.chain().setTextSelection(find(target, 'One').from).toggleBlockquote().run()
    },
  },
  {
    name: 'a rule and a line break',
    document: 'empty',
    markdown: 'a\nb\n\n---',
    edit: (target) => {
      target.commands.setTextSelection(1)
      type(target, 'a')
      target.commands.setHardBreak()
      type(target, 'b')
      target.commands.setHorizontalRule()
    },
  },
  {
    name: 'type in a quote in an item',
    document: 'blocks in items',
    markdown: '- first\n\n  second paragraph\n\n  ```\n  code in an item\n  ```\n- > a quote in an item, edited\n-\n',
    edit: (target) => {
      target.commands.setTextSelection(find(target, 'a quote in an item').to)
      type(target, ', edited')
    },
  },
  {
    name: 'make tasks of a list',
    document: 'lists',
    markdown: '- [ ] one\n- two\n  - nested\n\n3. three\n4. four\n\n- [ ] to do\n- [x] done\n',
    edit: (target) => {
      target.commands.setTextSelection(find(target, 'one').from)
      toggleTasks(target)
    },
  },
  {
    name: 'tasks from a paragraph',
    document: 'paragraphs',
    markdown: '- [ ] One line\n  and the next.\n\nA second paragraph with a hard  \nbreak.\n',
    edit: (target) => {
      target.commands.setTextSelection(find(target, 'One').from)
      toggleTasks(target)
    },
  },
  {
    name: 'type a task box',
    document: 'empty',
    markdown: '- [ ] milk',
    edit: (target) => {
      target.commands.setTextSelection(1)
      typeKeys(target, '[ ] milk')
    },
  },
  {
    name: 'type a list, then a done task',
    document: 'empty',
    markdown: '- [x] done',
    edit: (target) => {
      target.commands.setTextSelection(1)
      typeKeys(target, '- [x] done')
    },
  },
  {
    name: 'type a heading',
    document: 'empty',
    markdown: '## Plan',
    edit: (target) => {
      target.commands.setTextSelection(1)
      typeKeys(target, '## Plan')
    },
  },
  {
    name: 'a table from the toolbar',
    document: 'paragraphs',
    markdown: 'One line\nand the next.\n\nA second paragraph with a hard  \nbreak.\n\n|  |  |  |\n| --- | --- | --- |\n|  |  |  |\n|  |  |  |\n',
    edit: (target) => {
      target.commands.setTextSelection(find(target, 'break.').to)
      insertTable(target)
    },
  },
  {
    name: 'a link with nothing selected',
    document: 'empty',
    // A bare address reads back as a link.
    markdown: 'https://example.org/a',
    edit: (target) => {
      target.commands.setTextSelection(1)
      setLink(target, 'https://example.org/a')
    },
  },
  {
    name: 'italic inside bold',
    document: 'marks',
    markdown:
      'Some **_bold_**, _italic_, ~~struck~~, `code`, **bold `code`**, [a link](https://example.com), [a page](/projects) and AWD-12.\n',
    edit: (target) => {
      target.chain().setTextSelection(find(target, 'bold')).toggleItalic().run()
    },
  },
  {
    name: 'embed a view after a paragraph',
    document: 'paragraphs',
    markdown:
      'One line\nand the next.\n\n[Playtest feedback](/projects/p/collections/c?view=v "embed")\n\nA second paragraph with a hard  \nbreak.\n',
    edit: (target) => {
      target.commands.setTextSelection(find(target, 'One').from)
      insertEmbed(target, { href: '/projects/p/collections/c?view=v', label: 'Playtest feedback' })
    },
  },
  {
    name: 'embed a view on an empty page',
    document: 'empty',
    markdown: '[A \\[draft\\] view](/projects/p/collections/c "embed")',
    edit: (target) => {
      target.commands.setTextSelection(1)
      insertEmbed(target, { href: '/projects/p/collections/c', label: 'A [draft] view' })
    },
  },
  {
    name: 'remove an embed',
    document: 'embeds',
    markdown: 'What players said:\n\n[A \\[draft\\] view](/projects/p/collections/c "embed")\n',
    edit: (target) => {
      let at = -1
      target.state.doc.forEach((node, offset) => {
        if (at < 0 && node.type.name === 'embed') at = offset
      })
      target.chain().setNodeSelection(at).deleteSelection().run()
    },
  },
  {
    name: 'type below an embed',
    document: 'embeds',
    markdown:
      'What players said:\n\n[Playtest feedback](/projects/p/collections/c?view=v "embed")\n\nMore soon.\n\n[A \\[draft\\] view](/projects/p/collections/c "embed")\n',
    edit: (target) => {
      let after = -1
      target.state.doc.forEach((node, offset) => {
        if (after < 0 && node.type.name === 'embed') after = offset + node.nodeSize
      })
      target.chain().insertContentAt(after, { type: 'paragraph', content: [{ type: 'text', text: 'More soon.' }] }).run()
    },
  },
]

describe('edits the editor makes', () => {
  it('keeps an id on the text when Enter splits a block', () => {
    const { editor: opened } = open('paragraphs')
    opened.commands.setTextSelection(find(opened, 'A second').from)
    opened.commands.keyboardShortcut('Enter')
    const ids = opened.state.doc.content.content.map((block) => block.attrs.id as string | null)
    expect(ids).toEqual(['block0', null, 'block1'])
    expect(repeatedIds(opened.state.doc)).toEqual([])
  })

  it('leaves the id on the first half when a block splits in the middle', () => {
    const { editor: opened } = open('paragraphs')
    opened.commands.setTextSelection(find(opened, ' paragraph').from)
    opened.commands.keyboardShortcut('Enter')
    const ids = opened.state.doc.content.content.map((block) => block.attrs.id as string | null)
    expect(ids).toEqual(['block0', 'block1', null])
  })

  it('takes no unsafe link from a paste', () => {
    const { editor: opened } = open('empty')
    opened.commands.setTextSelection(1)
    // jsdom has no ClipboardEvent; the paste only passes it along.
    opened.view.pasteHTML(
      '<p><a href="javascript:alert(1)">bad</a> <a href=" https://ok.example/ ">good</a></p>',
      new Event('paste') as ClipboardEvent,
    )
    const links: string[] = []
    opened.state.doc.descendants((node) => {
      for (const mark of node.marks) if (mark.type.name === 'link') links.push(mark.attrs.href as string)
    })
    expect(links).toEqual(['https://ok.example/'])
  })

  it('keeps embeds at the top level, and takes none that leaves the app from a paste', () => {
    const { editor: opened } = open('lists')
    opened.commands.setTextSelection(find(opened, 'nested').from)
    insertEmbed(opened, { href: '/projects/p/collections/c', label: 'Bugs' })
    const top = opened.state.doc.content.content.map((block) => block.type.name)
    expect(top.filter((name) => name === 'embed')).toHaveLength(1)
    let nested = 0
    opened.state.doc.descendants((node, _position, parent) => {
      if (node.type.name === 'embed' && parent !== opened.state.doc) nested += 1
    })
    expect(nested).toBe(0)
    opened.destroy()

    const { editor: empty } = open('empty')
    empty.commands.setTextSelection(1)
    empty.view.pasteHTML(
      '<div data-embed="https://evil.example/x" data-label="Outside"></div><div data-embed="/projects/p/collections/d" data-label="Inside"></div>',
      new Event('paste') as ClipboardEvent,
    )
    const embeds: Array<[string, string]> = []
    empty.state.doc.forEach((node) => {
      if (node.type.name === 'embed') embeds.push([node.attrs.href as string, node.attrs.label as string])
    })
    expect(embeds).toEqual([['/projects/p/collections/d', 'Inside']])
  })

  it('records each edit as the update it sends', () => {
    const recorded = EDIT_CASES.map((edit) => {
      const opened = open(edit.document)
      edit.edit(opened.editor)
      expect(opened.errors, edit.name).toEqual([])
      expect(opened.updates.length, edit.name).toBeGreaterThan(0)
      const update = base64(Y.encodeStateAsUpdate(opened.doc, opened.server))
      opened.editor.destroy()
      editor = null
      return { name: edit.name, document: edit.document, markdown: edit.markdown, update }
    })
    const expected = { about: ABOUT, edits: recorded }
    if (process.env.UPDATE_DOC_PARITY) {
      writeFileSync(EDITS, `${JSON.stringify(expected, null, 2)}\n`)
      return
    }
    expect(JSON.parse(readFileSync(EDITS, 'utf8'))).toEqual(expected)
  })
})
