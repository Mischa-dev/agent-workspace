/**
 * Small text helpers every page uses, so counts and previews read the same
 * everywhere: "1 task" and "3 tasks", and Markdown shown as one plain line.
 */

/** "1 task", "3 tasks", "1,204 tasks". Pass the plural when it isn't the singular plus "s". */
export function countLabel(count: number, singular: string, plural = `${singular}s`) {
  return `${count.toLocaleString()} ${count === 1 ? singular : plural}`
}

const FENCE = /^\s*(```|~~~)[^\n]*\n([\s\S]*?)(?:\n\s*\1\s*(?:\n|$)|$)/gm

/**
 * Markdown as one line of plain text, for previews: list rows, toasts, quotes
 * and search results. Keeps the words and drops the syntax, so "**Ship** the
 * [build](https://…)" reads "Ship the build". Cuts at `maxLength` on a word
 * boundary with an ellipsis when it's given.
 */
export function plainPreview(markdown: string | null | undefined, maxLength = 0): string {
  if (!markdown) return ''
  let text = markdown.replace(/\r\n?/g, '\n')
  // Fenced code keeps its first line of content, the way a person would quote it.
  text = text.replace(FENCE, (_match, _fence: string, body: string) => `${body.split('\n').find((line) => line.trim()) ?? ''}\n`)
  text = text
    .split('\n')
    .map((line) =>
      line
        // Table rules and pipes.
        .replace(/^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/, '')
        .replace(/^\s*\|(.*)\|\s*$/, (_match, cells: string) => cells.split('|').map((cell) => cell.trim()).join(' '))
        // Headings, quotes, list markers and task boxes.
        .replace(/^\s{0,3}#{1,6}\s+/, '')
        .replace(/^\s*(>\s?)+/, '')
        .replace(/^\s*([-*+]|\d+[.)])\s+(\[[ xX]\]\s+)?/, '')
        // Horizontal rules.
        .replace(/^\s*([-*_])(\s*\1){2,}\s*$/, ''),
    )
    .join('\n')
  text = text
    // Images keep their alt text, links their words.
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]+)\]\((?:[^()]|\([^)]*\))*\)/g, '$1')
    .replace(/\[([^\]]+)\]\[[^\]]*\]/g, '$1')
    .replace(/<(https?:\/\/[^>\s]+)>/g, '$1')
    // Inline code, then emphasis and strike-through.
    .replace(/`+([^`]+)`+/g, '$1')
    .replace(/(\*\*|__)(?=\S)([\s\S]*?\S)\1/g, '$2')
    .replace(/(^|[^\w*])\*(?=\S)([^*\n]*?\S)\*(?!\w)/g, '$1$2')
    .replace(/(^|[^\w_])_(?=\S)([^_\n]*?\S)_(?!\w)/g, '$1$2')
    .replace(/~~(?=\S)([\s\S]*?\S)~~/g, '$1')
    // Stray HTML tags and escapes.
    .replace(/<\/?[a-zA-Z][^>]*>/g, '')
    .replace(/\\([\\`*_{}[\]()#+\-.!|>~])/g, '$1')
  const line = text.replace(/\s+/g, ' ').trim()
  if (!maxLength || line.length <= maxLength) return line
  const cut = line.slice(0, maxLength)
  const space = cut.lastIndexOf(' ')
  return `${(space > maxLength * 0.6 ? cut.slice(0, space) : cut).replace(/[\s.,;:!?-]+$/, '')}…`
}
