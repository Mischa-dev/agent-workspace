import type { Editor } from '@tiptap/core'
import type { Node as ProseMirrorNode } from '@tiptap/pm/model'
import type { EditorState } from '@tiptap/pm/state'

import { safeHref } from '../../ui/markdown/parse'

// What the page editor's toolbar does beyond Tiptap's own commands.

const LISTS = new Set(['bulletList', 'orderedList'])

/** The items of the innermost list the selection is in, with where each starts. */
export function selectedItems(state: EditorState): Array<{ node: ProseMirrorNode; position: number }> {
  const { $from, $to } = state.selection
  const range = $from.blockRange($to, (node) => LISTS.has(node.type.name))
  if (!range) return []
  const items: Array<{ node: ProseMirrorNode; position: number }> = []
  let position = range.start
  for (let index = range.startIndex; index < range.endIndex; index += 1) {
    const node = range.parent.child(index)
    items.push({ node, position })
    position += node.nodeSize
  }
  return items
}

const isTask = (node: ProseMirrorNode) => typeof node.attrs.checked === 'boolean'

/** Whether the selection is in tasks: list items with a box to tick. */
export function inTasks(state: EditorState) {
  const items = selectedItems(state)
  return items.length > 0 && items.every(({ node }) => isTask(node))
}

/**
 * Turns the selected list items into tasks, or back into plain items when
 * they all are; outside a list, starts a bulleted list of tasks.
 */
export function toggleTasks(editor: Editor) {
  let items = selectedItems(editor.state)
  if (items.length === 0) {
    if (!editor.chain().focus().toggleBulletList().run()) return false
    items = selectedItems(editor.state)
    if (items.length === 0) return false
  }
  const plain = items.every(({ node }) => isTask(node))
  const tr = editor.state.tr
  for (const { node, position } of items) {
    tr.setNodeAttribute(position, 'checked', plain ? null : isTask(node) ? node.attrs.checked : false)
  }
  editor.view.dispatch(tr)
  editor.commands.focus()
  return true
}

/** Where a table may go: not inside a list or quote, where Markdown can't hold one. */
export function canInsertTable(editor: Editor) {
  const { $from } = editor.state.selection
  for (let depth = $from.depth; depth > 0; depth -= 1) {
    const name = $from.node(depth).type.name
    if (name === 'listItem' || name === 'blockquote' || name === 'table') return false
  }
  return editor.can().insertTable({ rows: 3, cols: 3, withHeaderRow: true })
}

export function insertTable(editor: Editor) {
  return canInsertTable(editor) && editor.chain().focus().insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run()
}

/**
 * Puts an embed after the top-level block the cursor is in, or in place of
 * it when that's an empty paragraph: an embed sits only at the page's top
 * level, as its Markdown does.
 */
export function insertEmbed(editor: Editor, attributes: { href: string; label: string }) {
  const { selection } = editor.state
  const { $from } = selection
  const content = { type: 'embed', attrs: attributes }
  if ($from.depth === 0) return editor.chain().focus().insertContentAt(selection.to, content).run()
  const top = $from.node(1)
  const at = top.type.name === 'paragraph' && top.content.size === 0 ? { from: $from.before(1), to: $from.after(1) } : $from.after(1)
  return editor.chain().focus().insertContentAt(at, content).run()
}

/** The link the cursor is on, if any. */
export function currentLink(editor: Editor): string | null {
  const href: unknown = editor.getAttributes('link').href
  return typeof href === 'string' && editor.isActive('link') ? href : null
}

/**
 * Links the selection, or the link the cursor is on, to `address`; with
 * nothing selected, adds the address itself as linked text. False when the
 * address isn't one a page may link to.
 */
export function setLink(editor: Editor, address: string) {
  const safe = safeHref(address)
  if (!safe) return false
  if (editor.state.selection.empty && !editor.isActive('link')) {
    return editor
      .chain()
      .focus()
      .insertContent({ type: 'text', text: address.trim(), marks: [{ type: 'link', attrs: { href: safe.href } }] })
      .run()
  }
  return editor.chain().focus().extendMarkRange('link').setMark('link', { href: safe.href }).run()
}

export function removeLink(editor: Editor) {
  return editor.chain().focus().extendMarkRange('link').unsetMark('link').run()
}
