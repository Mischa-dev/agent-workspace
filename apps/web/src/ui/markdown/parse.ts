/**
 * A small, dependency-free Markdown parser. It produces a plain tree that the
 * renderer turns into React elements, so nothing in the source is ever treated
 * as HTML: `<script>` in a page is just text.
 *
 * Bounded on purpose: input past MAX_SOURCE is not parsed (it is shown as
 * plain text), containers nest at most MAX_BLOCK_DEPTH deep, emphasis and
 * links nest at most MAX_INLINE_DEPTH deep, and the inline scanner remembers
 * failed closer searches so a line of unmatched `*` stays linear.
 */

export type Inline =
  | { type: 'text'; value: string }
  | { type: 'strong' | 'em' | 'del'; children: Inline[] }
  | { type: 'code'; value: string }
  | { type: 'link'; href: string; external: boolean; children: Inline[] }
  | { type: 'taskKey'; key: string }
  | { type: 'br' }

export type Align = 'left' | 'center' | 'right' | null

export type ListItem = { checked: boolean | null; children: Block[] }

export type Block =
  | { type: 'heading'; level: 1 | 2 | 3 | 4 | 5 | 6; children: Inline[] }
  | { type: 'paragraph'; children: Inline[] }
  | { type: 'code'; lang: string | null; value: string }
  | { type: 'quote'; children: Block[] }
  | { type: 'list'; ordered: boolean; start: number; items: ListItem[] }
  | { type: 'hr' }
  | { type: 'table'; align: Align[]; head: Inline[][]; rows: Inline[][][] }
  /** Source past the parsing limit, shown as it is. */
  | { type: 'overflow'; value: string }

export const MAX_SOURCE = 100_000
const MAX_BLOCK_DEPTH = 8
const MAX_INLINE_DEPTH = 6
const MAX_TABLE_COLUMNS = 40

export type ParseOptions = {
  /** Whether a PROJECT-123 shaped word should become a task key. */
  isTaskKey?: (key: string) => boolean
}

export function parseMarkdown(source: string, options: ParseOptions = {}): Block[] {
  const normalized = source.replace(/\r\n?/g, '\n')
  const head = normalized.length > MAX_SOURCE ? normalized.slice(0, MAX_SOURCE) : normalized
  const blocks = parseBlocks(head.split('\n'), 0, options)
  if (head.length < normalized.length) blocks.push({ type: 'overflow', value: normalized.slice(MAX_SOURCE) })
  return blocks
}

// ---------------------------------------------------------------------------
// Links

/**
 * The href to use for a link destination, or null when it must not be a link.
 * Allowed: http:, https:, mailto: and same-origin paths starting with a single `/`.
 */
export function safeHref(raw: string): { href: string; external: boolean } | null {
  const href = raw.trim()
  if (!href || href.length > 2048) return null
  // Control characters, whitespace and backslashes let browsers reinterpret a URL.
  // eslint-disable-next-line no-control-regex
  if (/[\u0000- \u007f\\]/.test(href)) return null
  if (href.startsWith('/')) return href.startsWith('//') ? null : { href, external: false }
  const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(href)?.[1]?.toLowerCase()
  if (scheme === 'mailto') return href.length > 7 ? { href, external: true } : null
  if (scheme !== 'http' && scheme !== 'https') return null
  try {
    const url = new URL(href)
    return url.hostname ? { href, external: true } : null
  } catch {
    return null
  }
}

// ---------------------------------------------------------------------------
// Blocks

const FENCE = /^( {0,3})(`{3,}|~{3,})[ \t]*([^\s`]*)[^`]*$/
const HEADING = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$/
const RULE = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/
const QUOTE = /^ {0,3}> ?/
const LIST_ITEM = /^( {0,3})([-*+]|\d{1,9}[.)])(?:([ \t]+)(.*))?$/
const TABLE_DELIMITER = /^ {0,3}\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*$/
const TASK = /^\[([ xX])\](?:[ \t]+|$)/

const isBlank = (line: string) => /^[ \t]*$/.test(line)

/** Width of leading whitespace, with tabs as four columns. */
function indentOf(line: string) {
  let width = 0
  for (const char of line) {
    if (char === ' ') width += 1
    else if (char === '\t') width += 4 - (width % 4)
    else break
  }
  return width
}

/** Remove up to `width` columns of leading whitespace. */
function dedent(line: string, width: number) {
  let column = 0
  let index = 0
  while (index < line.length && column < width) {
    const char = line[index]
    if (char === ' ') column += 1
    else if (char === '\t') column += 4 - (column % 4)
    else break
    index += 1
  }
  return (column > width ? ' '.repeat(column - width) : '') + line.slice(index)
}

function startsBlock(line: string) {
  return FENCE.test(line) || HEADING.test(line) || RULE.test(line) || QUOTE.test(line) || LIST_ITEM.test(line)
}

function parseBlocks(lines: string[], depth: number, options: ParseOptions): Block[] {
  const blocks: Block[] = []
  let i = 0
  while (i < lines.length) {
    const line = lines[i]
    if (isBlank(line)) {
      i += 1
      continue
    }

    const fence = FENCE.exec(line)
    if (fence) {
      const [, indent, marker, info] = fence
      const body: string[] = []
      i += 1
      while (i < lines.length) {
        const closing = /^ {0,3}(`{3,}|~{3,})[ \t]*$/.exec(lines[i])
        if (closing && closing[1][0] === marker[0] && closing[1].length >= marker.length) {
          i += 1
          break
        }
        body.push(dedent(lines[i], indent.length))
        i += 1
      }
      blocks.push({ type: 'code', lang: info ? info.toLowerCase() : null, value: body.join('\n') })
      continue
    }

    const heading = HEADING.exec(line)
    if (heading) {
      const level = heading[1].length as 1 | 2 | 3 | 4 | 5 | 6
      blocks.push({ type: 'heading', level, children: parseInline(heading[2] ?? '', options) })
      i += 1
      continue
    }

    if (RULE.test(line)) {
      blocks.push({ type: 'hr' })
      i += 1
      continue
    }

    if (depth < MAX_BLOCK_DEPTH && QUOTE.test(line)) {
      const inner: string[] = []
      while (i < lines.length && QUOTE.test(lines[i])) {
        inner.push(lines[i].replace(QUOTE, ''))
        i += 1
      }
      blocks.push({ type: 'quote', children: parseBlocks(inner, depth + 1, options) })
      continue
    }

    if (depth < MAX_BLOCK_DEPTH && LIST_ITEM.test(line)) {
      const parsed = parseList(lines, i, depth, options)
      blocks.push(parsed.block)
      i = parsed.next
      continue
    }

    if (indentOf(line) >= 4) {
      const body: string[] = []
      while (i < lines.length && (indentOf(lines[i]) >= 4 || isBlank(lines[i]))) {
        body.push(dedent(lines[i], 4))
        i += 1
      }
      while (body.length && isBlank(body[body.length - 1])) body.pop()
      blocks.push({ type: 'code', lang: null, value: body.join('\n') })
      continue
    }

    if (line.includes('|') && i + 1 < lines.length && TABLE_DELIMITER.test(lines[i + 1])) {
      const table = parseTable(lines, i, options)
      if (table) {
        blocks.push(table.block)
        i = table.next
        continue
      }
    }

    const paragraph: string[] = [line]
    i += 1
    while (i < lines.length && !isBlank(lines[i]) && !startsBlock(lines[i])) {
      paragraph.push(lines[i])
      i += 1
    }
    blocks.push({ type: 'paragraph', children: parseInline(paragraph.map((part) => part.replace(/^[ \t]+/, '')).join('\n'), options) })
  }
  return blocks
}

function parseList(lines: string[], from: number, depth: number, options: ParseOptions) {
  const first = LIST_ITEM.exec(lines[from])!
  const ordered = /\d/.test(first[2][0])
  const kind = ordered ? first[2].slice(-1) : first[2]
  const start = ordered ? Number.parseInt(first[2], 10) : 1
  const items: ListItem[] = []
  let i = from
  while (i < lines.length) {
    const match = LIST_ITEM.exec(lines[i])
    if (!match) break
    const marker = match[2]
    const sameKind = ordered ? /\d/.test(marker[0]) && marker.slice(-1) === kind : marker === kind
    if (!sameKind || RULE.test(lines[i])) break
    const spacing = match[3] ?? ' '
    const spaces = indentOf(spacing)
    const contentIndent = match[1].length + marker.length + (spaces > 4 || !match[4] ? 1 : spaces)
    const body: string[] = [spaces > 4 ? dedent(spacing, 1) + (match[4] ?? '') : (match[4] ?? '')]
    i += 1
    while (i < lines.length) {
      const next = lines[i]
      if (isBlank(next)) {
        // A blank line stays in the item only when indented content follows.
        let ahead = i
        while (ahead < lines.length && isBlank(lines[ahead])) ahead += 1
        if (ahead < lines.length && indentOf(lines[ahead]) >= contentIndent) {
          for (; i < ahead; i += 1) body.push('')
          continue
        }
        break
      }
      if (indentOf(next) >= contentIndent) {
        body.push(dedent(next, contentIndent))
        i += 1
        continue
      }
      // Lazy continuation of the item's paragraph.
      if (!startsBlock(next) && !isBlank(body[body.length - 1])) {
        body.push(next.replace(/^[ \t]+/, ''))
        i += 1
        continue
      }
      break
    }
    let checked: boolean | null = null
    const task = TASK.exec(body[0])
    if (task) {
      checked = task[1] !== ' '
      body[0] = body[0].slice(task[0].length)
    }
    items.push({ checked, children: parseBlocks(body, depth + 1, options) })
    // Blank lines between items keep the list going.
    let ahead = i
    while (ahead < lines.length && isBlank(lines[ahead])) ahead += 1
    if (ahead < lines.length && ahead !== i && LIST_ITEM.test(lines[ahead])) i = ahead
  }
  return { block: { type: 'list' as const, ordered, start, items }, next: i }
}

function splitRow(line: string) {
  let row = line.trim()
  if (row.startsWith('|')) row = row.slice(1)
  if (row.endsWith('|') && !row.endsWith('\\|')) row = row.slice(0, -1)
  const cells: string[] = []
  let cell = ''
  let inCode = false
  for (let index = 0; index < row.length; index += 1) {
    const char = row[index]
    if (char === '\\' && row[index + 1] === '|') {
      cell += '|'
      index += 1
    } else if (char === '`') {
      inCode = !inCode
      cell += char
    } else if (char === '|' && !inCode) {
      cells.push(cell.trim())
      cell = ''
    } else {
      cell += char
    }
  }
  cells.push(cell.trim())
  return cells.slice(0, MAX_TABLE_COLUMNS)
}

function parseTable(lines: string[], from: number, options: ParseOptions) {
  const head = splitRow(lines[from])
  const delimiter = splitRow(lines[from + 1])
  if (head.length !== delimiter.length) return null
  const align: Align[] = delimiter.map((cell) => {
    const left = cell.startsWith(':')
    const right = cell.endsWith(':')
    return left && right ? 'center' : right ? 'right' : left ? 'left' : null
  })
  const rows: Inline[][][] = []
  let i = from + 2
  while (i < lines.length && !isBlank(lines[i]) && lines[i].includes('|') && !startsBlock(lines[i])) {
    const cells = splitRow(lines[i])
    rows.push(head.map((_, column) => parseInline(cells[column] ?? '', options)))
    i += 1
  }
  return {
    block: { type: 'table' as const, align, head: head.map((cell) => parseInline(cell, options)), rows },
    next: i,
  }
}

// ---------------------------------------------------------------------------
// Inlines

const PUNCTUATION = /[!-/:-@[-`{-~]/
/** A task key, matched where lastIndex is set (sticky). The page editor marks keys with it too. */
export const TASK_KEY = /[A-Z][A-Z0-9]{1,9}-[0-9]{1,9}(?![A-Za-z0-9_-])/y
const AUTOLINK = /https?:\/\/[^\s<>]+/iy
/** Common PROJECT-123 shaped words that are not task keys. */
export const NOT_TASK_KEYS = new Set(['UTF', 'ISO', 'SHA', 'RFC', 'GPT', 'COVID', 'CVE', 'MD', 'X', 'TLS', 'SSL', 'P', 'H', 'ES'])

const isWordChar = (char: string | undefined) => char !== undefined && /[A-Za-z0-9_]/.test(char)
const isSpace = (char: string | undefined) => char === undefined || /\s/.test(char)

type InlineContext = {
  options: ParseOptions
  /** Closer searches that already failed, keyed by marker; positions past this have none either. */
  noCloser: Map<string, number>
  /** Characters the link scanner may still look at; shared across the whole paragraph. */
  budget: { left: number }
}

export function parseInline(text: string, options: ParseOptions = {}): Inline[] {
  return inlineRun(text, 0, false, { options, noCloser: new Map(), budget: { left: 200_000 } })
}

function pushText(out: Inline[], value: string) {
  if (!value) return
  const last = out[out.length - 1]
  if (last?.type === 'text') last.value += value
  else out.push({ type: 'text', value })
}

/** Find a closing run of exactly `marker` at or after `from`. */
function findCloser(text: string, from: number, marker: string, context: InlineContext) {
  const failedAt = context.noCloser.get(marker)
  if (failedAt !== undefined && from >= failedAt) return -1
  const char = marker[0]
  let index = from
  while (index < text.length) {
    const found = text.indexOf(char, index)
    if (found < 0) break
    if (text[found - 1] === '\\') {
      index = found + 1
      continue
    }
    let end = found
    while (text[end] === char) end += 1
    const run = end - found
    const valid =
      run === marker.length &&
      !isSpace(text[found - 1]) &&
      found > from &&
      (char !== '_' || !isWordChar(text[end]))
    if (valid) return found
    index = end
  }
  context.noCloser.set(marker, Math.min(failedAt ?? Infinity, from))
  return -1
}

function inlineRun(text: string, depth: number, inLink: boolean, context: InlineContext): Inline[] {
  const out: Inline[] = []
  let index = 0
  let plain = ''
  const flush = () => {
    pushText(out, plain)
    plain = ''
  }
  while (index < text.length) {
    const char = text[index]

    if (char === '\\') {
      const next = text[index + 1]
      if (next === '\n') {
        flush()
        out.push({ type: 'br' })
        index += 2
        continue
      }
      if (next !== undefined && PUNCTUATION.test(next)) {
        plain += next
        index += 2
        continue
      }
    }

    if (char === '\n') {
      const hard = / {2,}$/.test(plain)
      plain = plain.replace(/[ \t]+$/, '')
      if (hard) {
        flush()
        out.push({ type: 'br' })
      } else {
        plain += '\n'
      }
      index += 1
      continue
    }

    if (char === '`') {
      let end = index
      while (text[end] === '`') end += 1
      const ticks = text.slice(index, end)
      const failedAt = context.noCloser.get(ticks)
      let close = failedAt !== undefined && end >= failedAt ? -1 : text.indexOf(ticks, end)
      while (close >= 0 && text[close + ticks.length] === '`') {
        let skip = close
        while (text[skip] === '`') skip += 1
        close = text.indexOf(ticks, skip)
      }
      if (close < 0 && failedAt === undefined) context.noCloser.set(ticks, end)
      if (close >= 0) {
        flush()
        let value = text.slice(end, close).replace(/\n/g, ' ')
        if (value.length > 2 && value.startsWith(' ') && value.endsWith(' ')) value = value.slice(1, -1)
        out.push({ type: 'code', value })
        index = close + ticks.length
      } else {
        plain += ticks
        index = end
      }
      continue
    }

    if (char === '[' && !inLink && depth < MAX_INLINE_DEPTH && context.budget.left > 0) {
      const link = readLink(text, index, context.budget)
      if (link) {
        flush()
        const safe = safeHref(link.destination)
        const children = inlineRun(link.label, depth + 1, true, { ...context, noCloser: new Map() })
        if (safe) out.push({ type: 'link', href: safe.href, external: safe.external, children })
        else out.push(...children)
        index = link.end
        continue
      }
    }

    if (char === '<' && !inLink) {
      const close = text.indexOf('>', index)
      const inside = close > index ? text.slice(index + 1, close) : ''
      const safe = /^(?:https?:\/\/|mailto:)[^\s<>]+$/i.test(inside) ? safeHref(inside) : null
      if (safe) {
        flush()
        out.push({ type: 'link', href: safe.href, external: safe.external, children: [{ type: 'text', value: inside }] })
        index = close + 1
        continue
      }
    }

    if ((char === 'h' || char === 'H') && !inLink && !isWordChar(text[index - 1])) {
      AUTOLINK.lastIndex = index
      const match = AUTOLINK.exec(text)
      if (match) {
        let url = match[0].replace(/[.,:;!?'"*_~]+$/, '')
        while (url.endsWith(')') && (url.match(/\)/g)?.length ?? 0) > (url.match(/\(/g)?.length ?? 0)) url = url.slice(0, -1)
        const safe = safeHref(url)
        if (safe) {
          flush()
          out.push({ type: 'link', href: safe.href, external: safe.external, children: [{ type: 'text', value: url }] })
          index += url.length
          continue
        }
      }
    }

    if (char >= 'A' && char <= 'Z' && !inLink && !/[A-Za-z0-9_-]/.test(text[index - 1] ?? ' ')) {
      TASK_KEY.lastIndex = index
      const match = TASK_KEY.exec(text)
      if (match) {
        const key = match[0]
        const project = key.slice(0, key.indexOf('-'))
        if (!NOT_TASK_KEYS.has(project) && (context.options.isTaskKey?.(key) ?? true)) {
          flush()
          out.push({ type: 'taskKey', key })
          index += key.length
          continue
        }
      }
    }

    if ((char === '*' || char === '_' || char === '~') && depth < MAX_INLINE_DEPTH) {
      let end = index
      while (text[end] === char) end += 1
      const run = end - index
      const opensWord = !isSpace(text[end]) && (char !== '_' || !isWordChar(text[index - 1]))
      const usable = char === '~' ? run === 2 : run <= 3
      if (opensWord && usable) {
        const marker = text.slice(index, end)
        const close = findCloser(text, end, marker, context)
        if (close >= 0) {
          flush()
          const inner = inlineRun(text.slice(end, close), depth + 1, inLink, { ...context, noCloser: new Map() })
          if (char === '~') out.push({ type: 'del', children: inner })
          else if (run === 1) out.push({ type: 'em', children: inner })
          else if (run === 2) out.push({ type: 'strong', children: inner })
          else out.push({ type: 'strong', children: [{ type: 'em', children: inner }] })
          index = close + run
          continue
        }
      }
      plain += text.slice(index, end)
      index = end
      continue
    }

    plain += char
    index += 1
  }
  flush()
  return out
}

/** Read `[label](destination "title")` starting at `[`. */
function readLink(text: string, start: number, budget: { left: number }) {
  let depth = 0
  let index = start
  let labelEnd = -1
  // Bounded scan so a long run of `[` cannot make this quadratic in practice.
  const limit = Math.min(text.length, start + 2000)
  for (; index < limit; index += 1) {
    const char = text[index]
    if (char === '\\') {
      index += 1
      continue
    }
    if (char === '[') depth += 1
    else if (char === ']') {
      depth -= 1
      if (depth === 0) {
        labelEnd = index
        break
      }
    }
  }
  budget.left -= index - start
  if (labelEnd < 0 || text[labelEnd + 1] !== '(') return null
  const destinationLimit = Math.min(text.length, labelEnd + 2050)
  let parens = 0
  for (let cursor = labelEnd + 2; cursor < destinationLimit; cursor += 1) {
    const char = text[cursor]
    if (char === '\n') return null
    if (char === '(') parens += 1
    else if (char === ')') {
      if (parens === 0) {
        const inside = text.slice(labelEnd + 2, cursor).trim()
        const destination = inside.replace(/\s+(?:"[^"]*"|'[^']*')$/, '').replace(/^<(.*)>$/, '$1')
        return { label: text.slice(start + 1, labelEnd), destination, end: cursor + 1 }
      }
      parens -= 1
    }
  }
  return null
}
