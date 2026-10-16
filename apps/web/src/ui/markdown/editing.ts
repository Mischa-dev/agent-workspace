/** Pure text edits behind the Markdown editor's toolbar and shortcuts. */

export type Selection = { value: string; start: number; end: number }

/** Replace value[from, to) with `insert`, then select [selectStart, selectEnd) in the new value. */
export type TextEdit = { from: number; to: number; insert: string; selectStart: number; selectEnd: number }

export type FormatAction = 'bold' | 'italic' | 'code' | 'link' | 'bullet' | 'checklist' | 'quote'

export function applyEdit(value: string, edit: TextEdit) {
  return value.slice(0, edit.from) + edit.insert + value.slice(edit.to)
}

function wrap({ end, start, value }: Selection, marker: string, placeholder: string): TextEdit {
  const selected = value.slice(start, end)
  const m = marker.length
  if (value.slice(start - m, start) === marker && value.slice(end, end + m) === marker) {
    return { from: start - m, to: end + m, insert: selected, selectStart: start - m, selectEnd: end - m }
  }
  if (selected.length > 2 * m && selected.startsWith(marker) && selected.endsWith(marker)) {
    const inner = selected.slice(m, -m)
    return { from: start, to: end, insert: inner, selectStart: start, selectEnd: start + inner.length }
  }
  const text = selected || placeholder
  return { from: start, to: end, insert: marker + text + marker, selectStart: start + m, selectEnd: start + m + text.length }
}

function code(selection: Selection): TextEdit {
  const { end, start, value } = selection
  const selected = value.slice(start, end)
  if (!selected.includes('\n')) return wrap(selection, '`', 'code')
  const before = start > 0 && value[start - 1] !== '\n' ? '\n' : ''
  const after = end < value.length && value[end] !== '\n' ? '\n' : ''
  const insert = `${before}\`\`\`\n${selected.replace(/\n$/, '')}\n\`\`\`${after}`
  const bodyStart = start + before.length + 4
  return { from: start, to: end, insert, selectStart: bodyStart, selectEnd: bodyStart + selected.replace(/\n$/, '').length }
}

function link({ end, start, value }: Selection): TextEdit {
  const selected = value.slice(start, end)
  if (/^(?:https?:\/\/|mailto:|\/)\S*$/i.test(selected)) {
    const insert = `[text](${selected})`
    return { from: start, to: end, insert, selectStart: start + 1, selectEnd: start + 5 }
  }
  const text = selected || 'text'
  const insert = `[${text}](https://)`
  const urlStart = start + text.length + 3
  return { from: start, to: end, insert, selectStart: urlStart, selectEnd: urlStart + 'https://'.length }
}

function prefixLines({ end, start, value }: Selection, prefix: string, existing: RegExp): TextEdit {
  const from = value.lastIndexOf('\n', start - 1) + 1
  const lineEnd = value.indexOf('\n', end > start && value[end - 1] === '\n' ? end - 1 : end)
  const to = lineEnd < 0 ? value.length : lineEnd
  const lines = value.slice(from, to).split('\n')
  const filled = lines.filter((line) => line.trim())
  const remove = filled.length > 0 && filled.every((line) => existing.test(line))
  const next = lines.map((line) => {
    if (remove) return line.replace(existing, '$1')
    if (!line.trim() && lines.length > 1) return line
    const indent = /^[ \t]*/.exec(line)?.[0] ?? ''
    return indent + prefix + line.slice(indent.length).replace(/^(?:[-*+] (?:\[[ xX]\] )?|> ?)/, '')
  })
  const insert = next.join('\n')
  const collapsed = start === end
  return {
    from,
    to,
    insert,
    selectStart: collapsed ? from + insert.length : from,
    selectEnd: from + insert.length,
  }
}

export function formatEdit(action: FormatAction, selection: Selection): TextEdit {
  switch (action) {
    case 'bold':
      return wrap(selection, '**', 'bold text')
    case 'italic':
      return wrap(selection, '_', 'italic text')
    case 'code':
      return code(selection)
    case 'link':
      return link(selection)
    case 'bullet':
      return prefixLines(selection, '- ', /^([ \t]*)[-*+] (?!\[[ xX]\] )/)
    case 'checklist':
      return prefixLines(selection, '- [ ] ', /^([ \t]*)[-*+] \[[ xX]\] /)
    case 'quote':
      return prefixLines(selection, '> ', /^([ \t]*)> ?/)
  }
}
