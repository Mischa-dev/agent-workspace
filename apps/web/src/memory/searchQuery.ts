/**
 * The search query grammar (C2.25), read the same way as the server's
 * `search/grammar.rs`; `apps/server/src/search/query_parity.json` holds the
 * cases both must agree on. Free words, "quoted phrases" and -excluded words
 * go to full-text search; `from:`, `in:`, `during:`, `on:`, `before:`,
 * `after:`, `has:`, `is:` and `kind:` narrow it.
 */

export type ParsedQuery = {
  text: string
  from: string[]
  inChannels: string[]
  inPeople: string[]
  after: string | null
  before: string | null
  has: string[]
  is: string[]
  kinds: string[]
  errors: string[]
}

/** One filter as typed, for a chip: its token, and whether it was understood. */
export type QueryFilter = { token: string; key: string; value: string; ok: boolean }

const KEYS = new Set(['from', 'in', 'during', 'on', 'before', 'after', 'has', 'is', 'kind'])

/** The query's tokens: runs of non-space, with a double-quoted part kept whole. */
export function queryTokens(query: string): string[] {
  const tokens: string[] = []
  let current = ''
  let quoted = false
  for (const character of query) {
    if (character === '"') {
      quoted = !quoted
      current += character
    } else if (/\s/.test(character) && !quoted) {
      if (current) tokens.push(current)
      current = ''
    } else {
      current += character
    }
  }
  if (current) tokens.push(current)
  return tokens
}

function filterOf(token: string): { key: string; value: string } | null {
  const at = token.indexOf(':')
  if (at < 0) return null
  const key = token.slice(0, at).toLowerCase()
  return KEYS.has(key) ? { key, value: token.slice(at + 1).toLowerCase() } : null
}

const push = (list: string[], value: string) => {
  if (!list.includes(value)) list.push(value)
}

function handle(value: string): string | null {
  const name = value.startsWith('@') ? value.slice(1) : value
  return name && [...name].length <= 80 && /^[\p{L}\p{N}_.-]+$/u.test(name) ? name : null
}

const KINDS: Record<string, string> = {
  message: 'messages',
  messages: 'messages',
  thread: 'threads',
  threads: 'threads',
  task: 'tasks',
  tasks: 'tasks',
  page: 'pages',
  pages: 'pages',
  decision: 'decisions',
  decisions: 'decisions',
  record: 'records',
  records: 'records',
  whiteboard: 'whiteboards',
  whiteboards: 'whiteboards',
  board: 'whiteboards',
  boards: 'whiteboards',
  sheet: 'sheets',
  sheets: 'sheets',
  file: 'files',
  files: 'files',
}

function daysIn(year: number, month: number): number {
  if (month === 2) return year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0) ? 29 : 28
  return [4, 6, 9, 11].includes(month) ? 30 : 31
}

const day = (year: number, month: number, date: number) =>
  `${String(year).padStart(4, '0')}-${String(month).padStart(2, '0')}-${String(date).padStart(2, '0')}`

/** A year, month or day as its first day and the day after its last. */
function span(value: string): [string, string] | null {
  const parts = value.split('-')
  if (parts.length > 3) return null
  const number = (part: string, width: number) => (part.length === width && /^\d+$/.test(part) ? Number(part) : null)
  const year = number(parts[0], 4)
  if (year === null || year < 1970 || year > 9998) return null
  let month: number | null = null
  if (parts.length > 1) {
    month = number(parts[1], 2)
    if (month === null || month < 1 || month > 12) return null
  }
  let date: number | null = null
  if (parts.length > 2 && month !== null) {
    date = number(parts[2], 2)
    if (date === null || date < 1 || date > daysIn(year, month)) return null
  }
  if (month === null) return [day(year, 1, 1), day(year + 1, 1, 1)]
  if (date === null) return month === 12 ? [day(year, 12, 1), day(year + 1, 1, 1)] : [day(year, month, 1), day(year, month + 1, 1)]
  const next = date < daysIn(year, month) ? day(year, month, date + 1) : month < 12 ? day(year, month + 1, 1) : day(year + 1, 1, 1)
  return [day(year, month, date), next]
}

function apply(parsed: ParsedQuery, key: string, value: string): boolean {
  const narrowAfter = (start: string) => {
    if (parsed.after === null || start > parsed.after) parsed.after = start
  }
  const narrowBefore = (end: string) => {
    if (parsed.before === null || end < parsed.before) parsed.before = end
  }
  switch (key) {
    case 'from': {
      const who = handle(value)
      if (!who) return false
      push(parsed.from, who)
      return true
    }
    case 'in': {
      if (value.startsWith('@')) {
        const who = handle(value.slice(1))
        if (!who) return false
        push(parsed.inPeople, who)
        return true
      }
      const name = value.startsWith('#') ? value.slice(1) : value
      if (!name || [...name].length > 80 || name.includes('"')) return false
      push(parsed.inChannels, name)
      return true
    }
    case 'during':
    case 'on': {
      const found = span(value)
      if (!found || (key === 'on' && value.length !== 10)) return false
      narrowAfter(found[0])
      narrowBefore(found[1])
      return true
    }
    case 'before': {
      const found = span(value)
      if (!found) return false
      narrowBefore(found[0])
      return true
    }
    case 'after': {
      const found = span(value)
      if (!found) return false
      narrowAfter(found[1])
      return true
    }
    case 'has': {
      const what = value.endsWith('s') ? value.slice(0, -1) : value
      if (!['file', 'link', 'reaction', 'pin', 'task'].includes(what)) return false
      push(parsed.has, what)
      return true
    }
    case 'is': {
      if (value === 'mine') push(parsed.from, 'me')
      else if (value === 'thread' || value === 'threads') push(parsed.is, 'thread')
      else if (value === 'dm' || value === 'dms') push(parsed.is, 'dm')
      else if (value === 'unread' || value === 'saved') push(parsed.is, value)
      else return false
      return true
    }
    case 'kind': {
      const kind = Object.hasOwn(KINDS, value) ? KINDS[value] : undefined
      if (!kind) return false
      push(parsed.kinds, kind)
      return true
    }
    default:
      return false
  }
}

export function parseSearchQuery(query: string): ParsedQuery {
  const parsed: ParsedQuery = {
    text: '',
    from: [],
    inChannels: [],
    inPeople: [],
    after: null,
    before: null,
    has: [],
    is: [],
    kinds: [],
    errors: [],
  }
  const words: string[] = []
  for (const token of queryTokens(query)) {
    const filter = filterOf(token)
    if (!filter) words.push(token)
    else if (!apply(parsed, filter.key, filter.value)) push(parsed.errors, token)
  }
  parsed.text = words.join(' ')
  return parsed
}

/** The query's filters as typed, each checked on its own, for chips. */
export function queryFilters(query: string): QueryFilter[] {
  return queryTokens(query).flatMap((token) => {
    const filter = filterOf(token)
    if (!filter) return []
    return [{ token, ...filter, ok: parseSearchQuery(token).errors.length === 0 }]
  })
}

/** The query without one of its tokens. */
export function withoutToken(query: string, token: string): string {
  const tokens = queryTokens(query)
  const at = tokens.indexOf(token)
  if (at >= 0) tokens.splice(at, 1)
  return tokens.join(' ')
}

/** The query with a filter added, unless it's there already. */
export function withToken(query: string, token: string): string {
  const tokens = queryTokens(query)
  if (tokens.some((existing) => existing.toLowerCase() === token.toLowerCase())) return tokens.join(' ')
  return [...tokens, token].join(' ')
}

/** A filter as words, for its chip. */
export function describeFilter(filter: QueryFilter): string {
  const { key, value } = filter
  if (!filter.ok) return filter.token
  switch (key) {
    case 'from':
      return value === 'me' ? 'From you' : `From @${value.replace(/^@/, '')}`
    case 'in':
      return value.startsWith('@') ? `DM with ${value}` : `In #${value.replace(/^#/, '')}`
    case 'during':
      return `During ${value}`
    case 'on':
      return `On ${value}`
    case 'before':
      return `Before ${value}`
    case 'after':
      return `After ${value}`
    case 'has':
      return `Has a ${value.replace(/s$/, '')}`
    case 'is':
      return { mine: 'From you', thread: 'In threads', threads: 'In threads', dm: 'In DMs', dms: 'In DMs', unread: 'Unread', saved: 'Saved' }[value] ?? value
    case 'kind':
      return `Only ${KINDS[value] ?? value}`
    default:
      return filter.token
  }
}
