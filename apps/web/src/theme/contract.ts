// The theme contract (contract.json): the tokens a theme may set, the
// built-in themes it may start from, the published part names its CSS may
// style, and the checks every theme passes. The server (src/themes.rs) runs
// the same checks before it stores a theme; these run before one is applied
// or imported, so a theme never loads anything from elsewhere.

import type { ThemeDocument } from '../api/generated/operations'
import contract from './contract.json'

export type { ThemeDocument }
export type ThemeModeName = 'light' | 'dark'
export type TokenKind = 'color' | 'type' | 'length' | 'shadow' | 'motion'
export type TokenGroup = { id: string; label: string; kind: TokenKind; tokens: string[] }

export const CONTRACT_VERSION: number = contract.version
export const TOKEN_GROUPS = contract.groups as TokenGroup[]
export const THEME_TOKENS: ReadonlySet<string> = new Set(TOKEN_GROUPS.flatMap((group) => group.tokens))
export const TOKEN_KIND: ReadonlyMap<string, TokenKind> = new Map(TOKEN_GROUPS.flatMap((group) => group.tokens.map((token) => [token, group.kind] as const)))
export const BASE_SCHEME: ReadonlyMap<string, ThemeModeName> = new Map(contract.presets.map((preset) => [preset.id, preset.scheme as ThemeModeName]))
export const PARTS = contract.parts

const TOKEN_WORDS: Record<string, string> = { bg: 'background', fg: 'text', t: 'type', on: 'text on' }

/** A token in plain words for the editor: "rail-bg" is "Rail background", "space-1-5" is "Space 1.5". */
export function tokenLabel(token: string): string {
  const words: string[] = []
  for (const part of token.split('-')) {
    const previous = words[words.length - 1]
    if (/^\d+$/.test(part) && previous && /^\d+$/.test(previous)) words[words.length - 1] = `${previous}.${part}`
    else words.push(TOKEN_WORDS[part] ?? part)
  }
  const text = words.join(' ')
  return text.charAt(0).toUpperCase() + text.slice(1)
}
export const LIMITS = contract.limits

const VALUE_FUNCTIONS = new Set(contract.valueFunctions.map((name) => name.toLowerCase()))
const CSS_FUNCTIONS = new Set(contract.cssFunctions.map((name) => name.toLowerCase()))
const CSS_AT_RULES = new Set(contract.cssAtRules)

/** Every function a piece of CSS calls, lowercased. A bare ( is grouping. */
function functions(text: string): string[] {
  return [...text.matchAll(/([A-Za-z0-9_-]+)\(/g)].map((match) => match[1].toLowerCase())
}

/** Quotes that close and brackets that balance; null when they do. */
function unbalanced(text: string): string | null {
  let quote: string | null = null
  const stack: string[] = []
  for (const character of text) {
    if (quote) {
      if (character === quote) quote = null
      else if (character === '\n') return 'Close every quote on the line it opens.'
      continue
    }
    if (character === '"' || character === "'") quote = character
    else if ('({['.includes(character)) stack.push(character)
    else if (')}]'.includes(character)) {
      const open = stack.pop()
      if (open !== { ')': '(', '}': '{', ']': '[' }[character]) return `A ${character} has nothing to close.`
    }
  }
  if (quote) return 'Close every quote.'
  if (stack.length) return 'Close every bracket.'
  return null
}

/** What's wrong with one token's value, or null. */
export function valueProblem(raw: string): string | null {
  const value = raw.trim()
  if (!value) return 'Give it a value, or leave the token out.'
  if ([...value].length > LIMITS.value) return `Keep a value to ${LIMITS.value} characters.`
  // eslint-disable-next-line no-control-regex
  const bad = /[;{}<>\\@!\u0000-\u001f\u007f]/.exec(value)
  if (bad) return `A value can't contain ${JSON.stringify(bad[0])}.`
  if (value.includes('/*')) return "A value can't contain a comment."
  const brackets = unbalanced(value)
  if (brackets) return brackets
  for (const name of functions(value)) {
    if (!VALUE_FUNCTIONS.has(name)) return `A value can't call ${name}(): use colors, lengths and math (calc, color-mix, var).`
  }
  for (const [, name] of value.matchAll(/--([A-Za-z0-9-]*)/g)) {
    if (!THEME_TOKENS.has(name) && name !== 'text-scale') return `var(--${name}) isn't a theme token.`
  }
  return null
}

function stripComments(css: string): string | null {
  let out = ''
  let quote: string | null = null
  for (let index = 0; index < css.length; index += 1) {
    const character = css[index]
    if (quote) {
      if (character === quote) quote = null
      out += character
    } else if (character === '/' && css[index + 1] === '*') {
      const end = css.indexOf('*/', index + 2)
      if (end < 0) return null
      out += ' '
      index = end + 1
    } else {
      if (character === '"' || character === "'") quote = character
      out += character
    }
  }
  return out
}

/** What's wrong with a theme's CSS, or null. */
export function cssProblem(css: string): string | null {
  if (!css) return null
  if (new TextEncoder().encode(css).length > LIMITS.css) return `Keep the CSS to ${LIMITS.css / 1024} KB.`
  if (css.includes('\\')) return "The CSS can't use backslash escapes."
  if (css.includes('<')) return "The CSS can't contain <: write > and + for selectors, and no markup."
  // eslint-disable-next-line no-control-regex
  const control = /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.exec(css)
  if (control) return `The CSS can't contain ${JSON.stringify(control[0])}.`
  const stripped = stripComments(css)
  if (stripped === null) return 'Close every comment.'
  const brackets = unbalanced(stripped)
  if (brackets) return brackets
  for (const [, name] of stripped.matchAll(/@([A-Za-z0-9-]*)/g)) {
    if (!CSS_AT_RULES.has(name.toLowerCase())) return `The CSS can't use @${name}: only @media, @supports, @container, @layer and @keyframes.`
  }
  for (const name of functions(stripped)) {
    if (!CSS_FUNCTIONS.has(name)) return `The CSS can't call ${name}(): a theme can't load anything (no url()).`
  }
  return null
}

/** Every problem in a document, by field ("light.tokens.accent", "css"); empty when it passes. */
export function documentProblems(document: ThemeDocument): Record<string, string> {
  const problems: Record<string, string> = {}
  if (document.format !== CONTRACT_VERSION) problems.format = `Write the theme for format ${CONTRACT_VERSION}.`
  for (const mode of ['light', 'dark'] as const) {
    const part = document[mode]
    if (BASE_SCHEME.get(part.base) !== mode) problems[`${mode}.base`] = `Start the ${mode} mode from a ${mode} built-in theme.`
    for (const [name, value] of Object.entries(part.tokens ?? {})) {
      const problem = THEME_TOKENS.has(name) ? valueProblem(value) : "That isn't a theme token."
      if (problem) problems[`${mode}.tokens.${name}`] = problem
    }
  }
  const css = cssProblem(document.css ?? '')
  if (css) problems.css = css
  return problems
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : null
}

/** A theme document read from anywhere (a file, the cache), or null when it isn't one or doesn't pass. */
export function readDocument(value: unknown): ThemeDocument | null {
  const candidate = record(value)
  if (!candidate) return null
  const mode = (part: unknown) => {
    const found = record(part)
    if (!found || typeof found.base !== 'string') return null
    const tokens: Record<string, string> = {}
    for (const [name, token] of Object.entries(record(found.tokens) ?? {})) {
      if (typeof token !== 'string') return null
      tokens[name] = token
    }
    return { base: found.base, tokens }
  }
  const light = mode(candidate.light)
  const dark = mode(candidate.dark)
  if (!light || !dark || typeof candidate.format !== 'number') return null
  const document: ThemeDocument = { format: candidate.format, light, dark, css: typeof candidate.css === 'string' ? candidate.css : '' }
  return Object.keys(documentProblems(document)).length ? null : document
}

/** A document for a built-in theme pair, changing nothing yet. */
export function blankDocument(light: string, dark: string): ThemeDocument {
  return { format: CONTRACT_VERSION, light: { base: light, tokens: {} }, dark: { base: dark, tokens: {} }, css: '' }
}
