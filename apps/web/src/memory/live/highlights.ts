import { Extension } from '@tiptap/core'
import type { Node as ProseMirrorNode } from '@tiptap/pm/model'
import { Plugin, PluginKey, type EditorState } from '@tiptap/pm/state'
import { Decoration, DecorationSet } from '@tiptap/pm/view'

// Comments and suggestions in the page editor (AWD-53): the words each open
// comment thread is about are marked, the active one more strongly, and a
// block an agent suggested changing is marked in the margin. Clicking marked
// words opens their thread. The page sends what to mark with setMeta; the
// document itself never changes.

/** An open thread's anchor: a top-level block's lasting id and the words in it. */
export type MarkedThread = { id: string; blockId: string; quote: string }

export type Marks = {
  threads: MarkedThread[]
  /** The thread whose words are shown as chosen. */
  active: string | null
  /** Blocks a pending suggestion changes. */
  suggested: string[]
}

export const discussionKey = new PluginKey<{ marks: Marks; decorations: DecorationSet }>('pageDiscussion')

const NONE: Marks = { threads: [], active: null, suggested: [] }

/** Text with every run of white space as one space, trimmed: as the server compares quotes. */
export function squash(text: string) {
  return text.split(/\s+/).filter(Boolean).join(' ')
}

/**
 * Where `quote` is in the top-level block at `offset`, comparing as the
 * server does: white space squashed, each paragraph or item on its own line.
 */
export function findQuote(block: ProseMirrorNode, offset: number, quote: string): { from: number; to: number } | null {
  const wanted = squash(quote)
  if (!wanted) return null
  // The block's words, one character at a time, each with its place in the
  // document (-1 for the line breaks between paragraphs).
  let text = ''
  const places: number[] = []
  const push = (character: string, place: number) => {
    const space = /\s/.test(character)
    if (space) {
      if (text.length === 0 || text.endsWith(' ')) return
      text += ' '
    } else {
      text += character
    }
    places.push(place)
  }
  let started = false
  block.descendants((node, position) => {
    if (node.isTextblock) {
      if (started) push('\n', -1)
      started = true
    }
    if (node.isText && node.text) {
      const start = offset + 1 + position
      for (let index = 0; index < node.text.length; index += 1) push(node.text[index], start + index)
    }
    return true
  })
  const at = text.indexOf(wanted)
  if (at < 0) return null
  const inside = places.slice(at, at + wanted.length).filter((place) => place >= 0)
  if (inside.length === 0) return null
  return { from: inside[0], to: inside[inside.length - 1] + 1 }
}

function decorate(doc: ProseMirrorNode, marks: Marks) {
  if (marks.threads.length === 0 && marks.suggested.length === 0) return DecorationSet.empty
  const decorations: Decoration[] = []
  const suggested = new Set(marks.suggested)
  const byBlock = new Map<string, MarkedThread[]>()
  for (const thread of marks.threads) byBlock.set(thread.blockId, [...(byBlock.get(thread.blockId) ?? []), thread])
  doc.forEach((block, offset) => {
    const id: unknown = block.attrs.id
    if (typeof id !== 'string') return
    if (suggested.has(id)) decorations.push(Decoration.node(offset, offset + block.nodeSize, { class: 'aw-live__suggested' }))
    for (const thread of byBlock.get(id) ?? []) {
      const found = findQuote(block, offset, thread.quote)
      if (!found) continue
      decorations.push(
        Decoration.inline(found.from, found.to, {
          class: thread.id === marks.active ? 'aw-live__comment is-active' : 'aw-live__comment',
          'data-thread': thread.id,
        }),
      )
    }
  })
  return DecorationSet.create(doc, decorations)
}

/** What a new comment on the selection is about, or why there's nothing to comment on. */
export function selectionAnchor(state: EditorState): { blockId: string; quote: string } | 'empty' | 'unsaved' {
  const { from, to, empty } = state.selection
  if (empty) return 'empty'
  const $from = state.doc.resolve(from)
  if ($from.depth < 1) return 'empty'
  const block = $from.node(1)
  const start = $from.before(1)
  const id: unknown = block.attrs.id
  // A block typed a moment ago gets its id when it reaches the server.
  if (typeof id !== 'string') return 'unsaved'
  const end = Math.min(to, start + block.nodeSize - 1)
  const quote = state.doc.textBetween(from, end, '\n').trim().slice(0, 1000).trim()
  if (!quote) return 'empty'
  return { blockId: id, quote }
}

export const DiscussionMarks = Extension.create<{ onOpenThread: (id: string) => void }>({
  name: 'pageDiscussion',

  addOptions() {
    return { onOpenThread: () => undefined }
  },

  addProseMirrorPlugins() {
    const open = (id: string) => this.options.onOpenThread(id)
    return [
      new Plugin({
        key: discussionKey,
        state: {
          init: (_, state) => ({ marks: NONE, decorations: decorate(state.doc, NONE) }),
          apply: (tr, value, _old, state) => {
            const marks = (tr.getMeta(discussionKey) as Marks | undefined) ?? value.marks
            if (marks === value.marks && !tr.docChanged) return value
            return { marks, decorations: decorate(state.doc, marks) }
          },
        },
        props: {
          decorations: (state) => discussionKey.getState(state)?.decorations ?? null,
          handleClick: (_view, _position, event) => {
            const target = event.target instanceof Element ? event.target.closest('[data-thread]') : null
            const id = target?.getAttribute('data-thread')
            if (id) open(id)
            // The cursor still goes where the click was.
            return false
          },
        },
      }),
    ]
  },
})
