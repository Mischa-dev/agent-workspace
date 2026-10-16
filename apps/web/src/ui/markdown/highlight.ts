/**
 * A small generic code tokenizer. Loaded with a dynamic import() only when a
 * page shows a fenced code block, so it never weighs on the first load.
 * It colours keywords, strings, comments, numbers and a few language-specific
 * shapes; everything else stays plain.
 */

export type TokenKind = 'keyword' | 'string' | 'comment' | 'number' | 'property' | 'plain'
export type Token = { kind: TokenKind; text: string }

type Rule = [TokenKind, RegExp]
type Grammar = { rules: Rule[]; keywords: Set<string>; caseInsensitive?: boolean }

const words = (list: string) => new Set(list.split(/\s+/).filter(Boolean))

const DOUBLE = /"(?:[^"\\\n]|\\.)*"?/y
const SINGLE = /'(?:[^'\\\n]|\\.)*'?/y
const BACKTICK = /`(?:[^`\\]|\\[\s\S])*`?/y
const BLOCK_COMMENT = /\/\*[\s\S]*?(?:\*\/|$)/y
const LINE_COMMENT = /\/\/[^\n]*/y
const HASH_COMMENT = /#[^\n]*/y
const NUMBER = /\b(?:0[xob][\da-f_]+|\d[\d_]*(?:\.\d[\d_]*)?(?:e[+-]?\d+)?)[a-z]*\b/iy
const WORD = /[A-Za-z_$][\w$]*/y

const JS_KEYWORDS = words(`
  as async await break case catch class const continue debugger default delete do else enum export extends false
  finally for from function get if implements import in instanceof interface let new null of private protected public
  readonly return satisfies set static super switch this throw true try type typeof undefined var void while with yield
`)

const GRAMMARS: Record<string, Grammar> = {
  js: {
    rules: [['comment', BLOCK_COMMENT], ['comment', LINE_COMMENT], ['string', DOUBLE], ['string', SINGLE], ['string', BACKTICK], ['number', NUMBER]],
    keywords: JS_KEYWORDS,
  },
  json: {
    rules: [['property', /"(?:[^"\\\n]|\\.)*"(?=\s*:)/y], ['string', DOUBLE], ['number', /-?\d+(?:\.\d+)?(?:e[+-]?\d+)?/iy]],
    keywords: words('true false null'),
  },
  rust: {
    rules: [
      ['comment', BLOCK_COMMENT],
      ['comment', LINE_COMMENT],
      ['string', /b?r#*"[\s\S]*?(?:"#*|$)/y],
      ['string', DOUBLE],
      ['string', /'(?:\\.|[^\\'\n])'/y],
      ['property', /#!?\[[^\]\n]*\]/y],
      ['number', NUMBER],
    ],
    keywords: words(`
      as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut
      pub ref return self Self static struct super trait true type unsafe use where while Some None Ok Err
    `),
  },
  python: {
    rules: [
      ['string', /[rbuf]{0,2}"""[\s\S]*?(?:"""|$)/iy],
      ['string', /[rbuf]{0,2}'''[\s\S]*?(?:'''|$)/iy],
      ['comment', HASH_COMMENT],
      ['string', DOUBLE],
      ['string', SINGLE],
      ['property', /@[\w.]+/y],
      ['number', NUMBER],
    ],
    keywords: words(`
      and as assert async await break class continue def del elif else except False finally for from global if import
      in is lambda None nonlocal not or pass raise return self True try while with yield match case
    `),
  },
  bash: {
    rules: [
      ['comment', /(?<![^\s;|&])#[^\n]*/y],
      ['string', DOUBLE],
      ['string', /'[^']*'?/y],
      ['property', /\$(?:\{[^}\n]*\}|[A-Za-z_]\w*|[@*#?$!0-9])/y],
      ['number', /\b\d+\b/y],
    ],
    keywords: words(`
      if then else elif fi for while until do done case esac in function return local export readonly set unset
      shift exit source alias echo cd sudo true false
    `),
  },
  sql: {
    rules: [['comment', /--[^\n]*/y], ['comment', BLOCK_COMMENT], ['string', /'(?:[^']|'')*'?/y], ['property', /"(?:[^"]|"")*"?/y], ['number', NUMBER]],
    keywords: words(`
      add all alter and as asc begin between by cascade case check column commit constraint create cross default
      delete desc distinct drop else end exists false foreign from full group having if in index inner insert into is
      join key left like limit not null offset on or order outer primary references returning right rollback select set
      table then true union unique update using values view when where with
    `),
    caseInsensitive: true,
  },
  css: {
    rules: [
      ['comment', BLOCK_COMMENT],
      ['string', DOUBLE],
      ['string', SINGLE],
      ['keyword', /@[\w-]+/y],
      ['number', /#[\da-f]{3,8}\b/iy],
      ['property', /--?[a-z][\w-]*(?=\s*:)/iy],
      ['property', /[a-z][\w-]*(?=\s*:[^:{;]*[;}\n])/iy],
      ['number', /-?\d*\.?\d+(?:[a-z]+|%)?/iy],
    ],
    keywords: words('important inherit initial unset none auto'),
  },
  html: {
    rules: [
      ['comment', /<!--[\s\S]*?(?:-->|$)/y],
      ['keyword', /<\/?[A-Za-z][\w:-]*|\/?>/y],
      ['property', /[A-Za-z_:][\w:.-]*(?==)/y],
      ['string', DOUBLE],
      ['string', SINGLE],
    ],
    keywords: new Set(),
  },
  yaml: {
    rules: [
      ['comment', /(?<![^\s])#[^\n]*/y],
      ['property', /(?<=^|\n)[ \t-]*[\w.-]+(?=[ \t]*:(?:\s|$))/y],
      ['string', DOUBLE],
      ['string', SINGLE],
      ['number', /-?\b\d+(?:\.\d+)?\b/y],
    ],
    keywords: words('true false null yes no on off'),
  },
}

const ALIASES: Record<string, string> = {
  javascript: 'js',
  jsx: 'js',
  mjs: 'js',
  cjs: 'js',
  ts: 'js',
  typescript: 'js',
  tsx: 'js',
  rs: 'rust',
  py: 'python',
  sh: 'bash',
  shell: 'bash',
  zsh: 'bash',
  console: 'bash',
  postgres: 'sql',
  postgresql: 'sql',
  psql: 'sql',
  scss: 'css',
  xml: 'html',
  svg: 'html',
  htm: 'html',
  yml: 'yaml',
  jsonc: 'json',
}

/** Highlighting stops past this many characters; the rest stays plain. */
const MAX_HIGHLIGHT = 50_000

export function grammarFor(language: string | null) {
  if (!language) return null
  const name = language.toLowerCase()
  return GRAMMARS[ALIASES[name] ?? name] ?? null
}

export function highlight(code: string, language: string | null): Token[] | null {
  const grammar = grammarFor(language)
  if (!grammar) return null
  const source = code.length > MAX_HIGHLIGHT ? code.slice(0, MAX_HIGHLIGHT) : code
  const tokens: Token[] = []
  const push = (kind: TokenKind, text: string) => {
    const last = tokens[tokens.length - 1]
    if (last && last.kind === kind) last.text += text
    else tokens.push({ kind, text })
  }
  let index = 0
  outer: while (index < source.length) {
    for (const [kind, pattern] of grammar.rules) {
      pattern.lastIndex = index
      const match = pattern.exec(source)
      if (match && match[0].length > 0) {
        push(kind, match[0])
        index += match[0].length
        continue outer
      }
    }
    WORD.lastIndex = index
    const word = WORD.exec(source)
    if (word) {
      const value = grammar.caseInsensitive ? word[0].toLowerCase() : word[0]
      push(grammar.keywords.has(value) ? 'keyword' : 'plain', word[0])
      index += word[0].length
      continue
    }
    push('plain', source[index])
    index += 1
  }
  if (source.length < code.length) push('plain', code.slice(source.length))
  return tokens
}
