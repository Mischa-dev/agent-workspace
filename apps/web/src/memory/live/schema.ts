import { Extension, InputRule, Mark, Node as TiptapNode, mergeAttributes, type Extensions } from '@tiptap/core'
import { Blockquote } from '@tiptap/extension-blockquote'
import { Bold } from '@tiptap/extension-bold'
import { Code } from '@tiptap/extension-code'
import { CodeBlock } from '@tiptap/extension-code-block'
import { Document } from '@tiptap/extension-document'
import { HardBreak } from '@tiptap/extension-hard-break'
import { Heading } from '@tiptap/extension-heading'
import { HorizontalRule } from '@tiptap/extension-horizontal-rule'
import { Italic } from '@tiptap/extension-italic'
// Each extension from its own entry, so the packages' others stay out of the chunk.
import { BulletList } from '@tiptap/extension-list/bullet-list'
import { ListItem } from '@tiptap/extension-list/item'
import { ListKeymap } from '@tiptap/extension-list/keymap'
import { OrderedList } from '@tiptap/extension-list/ordered-list'
import { Paragraph } from '@tiptap/extension-paragraph'
import { Strike } from '@tiptap/extension-strike'
import { TableCell } from '@tiptap/extension-table/cell'
import { TableHeader } from '@tiptap/extension-table/header'
import { TableRow } from '@tiptap/extension-table/row'
import { Table } from '@tiptap/extension-table/table'
import { Text } from '@tiptap/extension-text'
import { Dropcursor } from '@tiptap/extensions/drop-cursor'
import { Gapcursor } from '@tiptap/extensions/gap-cursor'
import { Placeholder } from '@tiptap/extensions/placeholder'
import type { Node as ProseMirrorNode } from '@tiptap/pm/model'
import { Plugin, PluginKey, type EditorState } from '@tiptap/pm/state'
import { Decoration, DecorationSet } from '@tiptap/pm/view'

import { highlight } from '../../ui/markdown/highlight'
import { embedHref, NOT_TASK_KEYS, safeHref, TASK_KEY } from '../../ui/markdown/parse'
import type { EmbedSlots } from './embeds'
import '../../styles/conversation.css'
import '../../styles/markdown.css'

// The page editor's document, in exactly the shape the server keeps it
// (apps/server/src/page_docs/ydoc.rs): the same node and mark names, the same
// attributes, and content rules that take every document the server makes
// from Markdown. A node or mark the server doesn't know would make the server
// refuse the edit, and one this schema doesn't know would stop the editor, so
// both sides check the same documents (pageDocuments.json, pageEdits.json).

/** Blocks with a lasting id, which agents use to edit them. */
export const BLOCK_TYPES = [
  'paragraph',
  'heading',
  'codeBlock',
  'blockquote',
  'bulletList',
  'orderedList',
  'horizontalRule',
  'table',
  'embed',
] as const

const blockTypes: readonly string[] = BLOCK_TYPES

/**
 * Blocks that repeat an earlier block's id, as splitting a block leaves
 * them: the id stays with the first of them that has something in it, so
 * Enter at the start of a paragraph leaves the id on its text. The server
 * gives the others new ids.
 */
export function repeatedIds(doc: ProseMirrorNode): number[] {
  const byId = new Map<string, Array<{ position: number; filled: boolean }>>()
  doc.descendants((node, position) => {
    const id: unknown = node.attrs.id
    if (typeof id === 'string' && blockTypes.includes(node.type.name)) {
      const blocks = byId.get(id) ?? []
      blocks.push({ position, filled: node.isLeaf || node.content.size > 0 })
      byId.set(id, blocks)
    }
    // Text holds no blocks.
    return !node.isTextblock
  })
  const repeats: number[] = []
  for (const blocks of byId.values()) {
    if (blocks.length < 2) continue
    const keeper = blocks.find((block) => block.filled) ?? blocks[0]
    for (const block of blocks) if (block !== keeper) repeats.push(block.position)
  }
  return repeats
}

const BlockIds = Extension.create({
  name: 'blockIds',

  addGlobalAttributes() {
    return [
      {
        types: [...BLOCK_TYPES],
        attributes: {
          id: {
            default: null,
            keepOnSplit: false,
            // A pasted block is a new block: the server gives it an id.
            parseHTML: () => null,
            renderHTML: (attributes) => (typeof attributes.id === 'string' ? { 'data-block-id': attributes.id } : {}),
          },
        },
      },
    ]
  },

  addProseMirrorPlugins() {
    return [
      new Plugin({
        key: new PluginKey('blockIds'),
        appendTransaction: (transactions, _old, state) => {
          if (!transactions.some((transaction) => transaction.docChanged)) return null
          const repeats = repeatedIds(state.doc)
          if (repeats.length === 0) return null
          const tr = state.tr
          for (const position of repeats) tr.setNodeAttribute(position, 'id', null)
          return tr
        },
      }),
    ]
  },
})

/** A link, with only its address: the server allows the addresses a rendered page links. */
const Link = Mark.create({
  name: 'link',
  priority: 1000,
  inclusive: false,

  addAttributes() {
    return {
      href: {
        default: null,
        parseHTML: (element) => safeHref(element.getAttribute('href') ?? '')?.href ?? null,
      },
    }
  },

  parseHTML() {
    return [{ tag: 'a[href]', getAttrs: (element) => (safeHref(element.getAttribute('href') ?? '') ? null : false) }]
  },

  renderHTML({ HTMLAttributes }) {
    const href = typeof HTMLAttributes.href === 'string' ? HTMLAttributes.href : ''
    const external = !href.startsWith('/')
    return ['a', mergeAttributes(HTMLAttributes, external ? { rel: 'noreferrer noopener', target: '_blank' } : {}), 0]
  },
})

/** The item an editor's selection is in, if any. */
function itemAt(state: EditorState, name: string): ProseMirrorNode | null {
  const { $from } = state.selection
  for (let depth = $from.depth; depth > 0; depth -= 1) {
    if ($from.node(depth).type.name === name) return $from.node(depth)
  }
  return null
}

/**
 * A list item, which can be a task: `checked` is true or false on a task and
 * null otherwise. It holds any blocks, as Markdown lets it.
 */
const PageListItem = ListItem.extend({
  content: 'block+',

  addAttributes() {
    return {
      checked: {
        default: null,
        keepOnSplit: true,
        parseHTML: (element) => {
          const value = element.getAttribute('data-checked')
          return value === 'true' ? true : value === 'false' ? false : null
        },
        renderHTML: (attributes) =>
          typeof attributes.checked === 'boolean' ? { class: 'aw-md__task', 'data-checked': String(attributes.checked) } : {},
      },
    }
  },

  addKeyboardShortcuts() {
    return {
      ...this.parent?.(),
      // A new task after a task, done or not, starts not done.
      Enter: () => {
        const item = itemAt(this.editor.state, this.name)
        return this.editor.commands.splitListItem(this.name, typeof item?.attrs.checked === 'boolean' ? { checked: false } : {})
      },
    }
  },

  addInputRules() {
    return [
      // Typing "[ ] " or "[x] " at the start of an item makes it a task, as in
      // Markdown; at the start of a paragraph, it starts a list of tasks.
      new InputRule({
        find: /^\[([ xX])\]\s$/,
        handler: ({ state, range, match, chain }) => {
          const checked = match[1] !== ' '
          const $start = state.doc.resolve(range.from)
          const first = $start.depth >= 2 && $start.node(-1).type.name === this.name && $start.index(-1) === 0
          if (first) state.tr.delete(range.from, range.to)
          else if ($start.depth !== 1 || !chain().deleteRange(range).toggleBulletList().run()) return null
          const { $from } = state.selection
          for (let depth = $from.depth; depth > 0; depth -= 1) {
            if ($from.node(depth).type.name === this.name) {
              state.tr.setNodeAttribute($from.before(depth), 'checked', checked)
              return
            }
          }
          return null
        },
      }),
    ]
  },

  addNodeView() {
    return ({ node, getPos, editor }) => {
      let current = node
      const dom = document.createElement('li')
      // The box sits outside the editable text, so a click ticks it.
      const holder = document.createElement('span')
      holder.contentEditable = 'false'
      holder.className = 'aw-live__box'
      const box = document.createElement('input')
      box.type = 'checkbox'
      holder.append(box)
      const content = document.createElement('div')
      content.className = 'aw-live__item'
      dom.append(holder, content)
      const draw = () => {
        const task = typeof current.attrs.checked === 'boolean'
        dom.classList.toggle('aw-md__task', task)
        holder.hidden = !task
        box.checked = current.attrs.checked === true
        box.disabled = !editor.isEditable
        box.setAttribute('aria-label', box.checked ? 'Done' : 'Not done')
      }
      draw()
      // Ticking a box doesn't move the cursor.
      box.addEventListener('mousedown', (event) => event.preventDefault())
      box.addEventListener('change', () => {
        const position = getPos()
        if (typeof position !== 'number' || !editor.isEditable) {
          draw()
          return
        }
        editor.view.dispatch(editor.state.tr.setNodeAttribute(position, 'checked', box.checked))
      })
      return {
        dom,
        contentDOM: content,
        update: (updated) => {
          if (updated.type !== current.type) return false
          current = updated
          draw()
          return true
        },
        stopEvent: (event) => event.target instanceof Node && holder.contains(event.target),
        ignoreMutation: (mutation) =>
          mutation.type !== 'selection' &&
          (holder.contains(mutation.target) || (mutation.type === 'attributes' && mutation.target === dom)),
      }
    }
  },
})

/**
 * Something of the app's shown live in the page, such as a collection's view
 * (parse.ts's embed): a block of its own at the page's top level, with the
 * path it shows and a label for while it loads. The page draws the view into
 * the element this keeps for it (EmbedSlots); without one, the label shows.
 */
const Embed = TiptapNode.create<{ slots: EmbedSlots | null }>({
  name: 'embed',
  group: 'embed',
  atom: true,
  selectable: true,
  draggable: true,

  addOptions() {
    return { slots: null }
  },

  addAttributes() {
    return {
      href: {
        default: null,
        parseHTML: (element) => embedHref(element.getAttribute('data-embed') ?? ''),
        renderHTML: (attributes) => (typeof attributes.href === 'string' ? { 'data-embed': attributes.href } : {}),
      },
      label: {
        default: null,
        parseHTML: (element) => element.getAttribute('data-label') || null,
        renderHTML: (attributes) => (typeof attributes.label === 'string' && attributes.label ? { 'data-label': attributes.label } : {}),
      },
    }
  },

  parseHTML() {
    return [{ tag: 'div[data-embed]', getAttrs: (element) => (embedHref(element.getAttribute('data-embed') ?? '') ? null : false) }]
  },

  renderHTML({ HTMLAttributes }) {
    return ['div', mergeAttributes(HTMLAttributes, { class: 'aw-embed' })]
  },

  addNodeView() {
    const { slots } = this.options
    return ({ node }) => {
      const dom = document.createElement('div')
      dom.className = 'aw-embed'
      dom.contentEditable = 'false'
      const show = (current: ProseMirrorNode) => {
        const href = typeof current.attrs.href === 'string' ? current.attrs.href : ''
        const label = typeof current.attrs.label === 'string' ? current.attrs.label : ''
        if (slots) slots.set({ element: dom, href, label })
        else dom.textContent = label || href
      }
      show(node)
      return {
        dom,
        update: (updated) => {
          if (updated.type !== node.type) return false
          show(updated)
          return true
        },
        selectNode: () => dom.classList.add('is-selected'),
        deselectNode: () => dom.classList.remove('is-selected'),
        // Links and buttons in the view work as themselves; a click anywhere else selects the embed.
        stopEvent: (event) => event.target instanceof Element && event.target.closest('a, button, input, select, textarea') !== null,
        ignoreMutation: () => true,
        destroy: () => slots?.remove(dom),
      }
    }
  },
})

/** A table without a column sizer or inline styles (the page's styles say how it looks). */
const PageTable = Table.extend({
  renderHTML({ HTMLAttributes }) {
    return ['div', mergeAttributes(HTMLAttributes, { class: 'aw-md__table' }), ['table', ['tbody', 0]]]
  },
})

const ALIGNS = new Set(['left', 'center', 'right'])

function cellAttributes() {
  return {
    colspan: { default: 1 },
    rowspan: { default: 1 },
    align: {
      default: null,
      parseHTML: (element: HTMLElement) => {
        const align = (element.getAttribute('data-align') || element.style.textAlign || element.getAttribute('align') || '').toLowerCase()
        return ALIGNS.has(align) ? align : null
      },
      renderHTML: (attributes: Record<string, unknown>) =>
        typeof attributes.align === 'string' ? { 'data-align': attributes.align } : {},
    },
  }
}

/** A cell holds one paragraph, as a Markdown table cell does. */
const PageCell = TableCell.extend({ content: 'paragraph', addAttributes: cellAttributes })
const PageHeaderCell = TableHeader.extend({ content: 'paragraph', addAttributes: cellAttributes })

/** The task keys in a run of plain text, where the rendered page would link them (parse.ts's parseInline). */
function findTaskKeys(text: string): Array<{ index: number; key: string }> {
  const found: Array<{ index: number; key: string }> = []
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index]
    if (char < 'A' || char > 'Z' || /[A-Za-z0-9_-]/.test(text[index - 1] ?? ' ')) continue
    TASK_KEY.lastIndex = index
    const key = TASK_KEY.exec(text)?.[0]
    if (key && !NOT_TASK_KEYS.has(key.slice(0, key.indexOf('-')))) {
      found.push({ index, key })
      index += key.length - 1
    }
  }
  return found
}

/** Colours code and marks task keys the way a rendered page does. */
function decorate(doc: ProseMirrorNode): DecorationSet {
  const decorations: Decoration[] = []
  doc.descendants((node, position) => {
    if (node.type.name === 'codeBlock') {
      const language = typeof node.attrs.language === 'string' ? node.attrs.language : null
      let at = position + 1
      for (const token of (language && highlight(node.textContent, language)) || []) {
        if (token.kind !== 'plain' && token.text) {
          decorations.push(Decoration.inline(at, at + token.text.length, { class: `aw-md-tok aw-md-tok--${token.kind}` }))
        }
        at += token.text.length
      }
      return false
    }
    if (node.isText && node.text && !node.marks.some((mark) => mark.type.name === 'code' || mark.type.name === 'link')) {
      for (const { index, key } of findTaskKeys(node.text)) {
        decorations.push(
          Decoration.inline(position + index, position + index + key.length, { class: 'aw-md__task-key', 'data-task-key': key }),
        )
      }
    }
    return true
  })
  return DecorationSet.create(doc, decorations)
}

const decorationsKey = new PluginKey<DecorationSet>('pageDecorations')

export type PageExtensionsOptions = {
  /** Opens a link or a task key, from a click with Cmd or Ctrl held (or any click when reading). */
  onOpen?: (href: string) => void
  placeholder?: string
  /** Where embeds' live views go. */
  embeds?: EmbedSlots
}

const PageLook = Extension.create<PageExtensionsOptions>({
  name: 'pageLook',

  addOptions() {
    return { onOpen: undefined, placeholder: undefined, embeds: undefined }
  },

  addProseMirrorPlugins() {
    const { onOpen } = this.options
    return [
      new Plugin({
        key: decorationsKey,
        state: {
          init: (_config, state) => decorate(state.doc),
          apply: (tr, set) => (tr.docChanged ? decorate(tr.doc) : set),
        },
        props: {
          decorations: (state) => decorationsKey.getState(state),
          handleClick: (view, _position, event) => {
            if (!onOpen || !(event.target instanceof Element)) return false
            const target = event.target.closest('a[href], [data-task-key]')
            if (!target || !view.dom.contains(target)) return false
            if (view.editable && !(event.metaKey || event.ctrlKey)) return false
            const key = target.getAttribute('data-task-key')
            const href = key ? `/${key}` : target.getAttribute('href')
            if (!href) return false
            event.preventDefault()
            onOpen(href)
            return true
          },
        },
      }),
    ]
  },
})

/** Everything the page editor's document is made of, without the live connection. */
export function pageExtensions(options: PageExtensionsOptions = {}): Extensions {
  return [
    // Embeds sit only at the top level, as their Markdown does.
    Document.extend({ content: '(block | embed)+' }),
    Paragraph,
    Text,
    Heading,
    Blockquote,
    BulletList,
    OrderedList,
    PageListItem,
    ListKeymap,
    CodeBlock.configure({ HTMLAttributes: { class: 'aw-md__pre' } }),
    HorizontalRule,
    Embed.configure({ slots: options.embeds ?? null }),
    HardBreak,
    PageTable.configure({ View: null, resizable: false, allowTableNodeSelection: false }),
    TableRow,
    PageCell,
    PageHeaderCell,
    Bold,
    Italic,
    Strike,
    // Code can sit inside bold or a link, as in **`this`**.
    Code.extend({ excludes: 'code' }).configure({ HTMLAttributes: { class: 'aw-md__code' } }),
    Link,
    BlockIds,
    Dropcursor,
    Gapcursor,
    Placeholder.configure({ placeholder: options.placeholder ?? 'Write here' }),
    PageLook.configure(options),
  ]
}
