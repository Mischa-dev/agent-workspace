// Validation and dropdowns on the grid (C1.15). The server keeps the rules
// and refuses what breaks a strict one; these mirror its reading so the grid
// can offer a dropdown or a checkbox and mark a cell that breaks a lenient one.
import type { ValidationRule } from './api'
import { parseRange } from './sheetModel'

/** Whether a rule covers a cell (rows and columns from 1). */
export function covers(rule: ValidationRule, row: number, column: number) {
  const range = parseRange(rule.range)
  return !!range && row >= range.top && row <= range.bottom && column >= range.left && column <= range.right
}

/** A tab's rules read once, for asking about many cells. */
export function ruleBook(rules: ValidationRule[]) {
  const parsed = rules.flatMap((rule) => {
    const range = parseRange(rule.range)
    return range ? [{ rule, range }] : []
  })
  const at = (row: number, column: number) =>
    parsed.filter(({ range }) => row >= range.top && row <= range.bottom && column >= range.left && column <= range.right).map(({ rule }) => rule)
  return {
    empty: parsed.length === 0,
    /** The rule that shapes how a cell is edited: the first list or checkbox on it. */
    control: (row: number, column: number) => (parsed.length ? (at(row, column).find((rule) => rule.allow === 'list' || rule.allow === 'checkbox') ?? null) : null),
    /** The first rule a cell's input breaks (a lenient one, since strict ones refuse). */
    broken: (row: number, column: number, input: string) => (parsed.length ? (at(row, column).find((rule) => !allows(rule, input)) ?? null) : null),
  }
}

/** A typed number as the sheet reads one: `1,200`, `$5`, `10%`. */
function number(text: string) {
  let body = text.trim()
  let scale = 1
  if (body.endsWith('%')) {
    body = body.slice(0, -1).trim()
    scale = 0.01
  }
  body = body.replace(/^[$€£]+/, '').replace(/,/g, '')
  if (!/^[+-]?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?$/i.test(body)) return null
  const value = Number(body)
  return Number.isFinite(value) ? value * scale : null
}

function date(text: string) {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text.trim())
  if (!match) return null
  const value = new Date(Date.UTC(Number(match[1]), Number(match[2]) - 1, Number(match[3])))
  return value.getUTCMonth() === Number(match[2]) - 1 ? value.getTime() : null
}

/** Whether `input` (as typed) is allowed. Empty cells and formulas are. */
export function allows(rule: ValidationRule, input: string) {
  const text = input.trim()
  if (!text || input.startsWith('=')) return true
  switch (rule.allow) {
    case 'list':
      return (rule.choices ?? []).some((choice) => choice.trim().toLowerCase() === text.toLowerCase())
    case 'checkbox':
      return /^(true|false)$/i.test(text)
    case 'number': {
      const value = number(text)
      return value !== null && (!rule.whole || Number.isInteger(value)) && (rule.min === undefined || value >= rule.min) && (rule.max === undefined || value <= rule.max)
    }
    case 'date': {
      const value = date(text)
      const from = rule.from ? date(rule.from) : null
      const to = rule.to ? date(rule.to) : null
      return value !== null && (from === null || value >= from) && (to === null || value <= to)
    }
    case 'textLength': {
      const length = [...text].length
      return (rule.min === undefined || length >= rule.min) && (rule.max === undefined || length <= rule.max)
    }
    default:
      return true
  }
}

/** What a rule allows, in words. */
export function describeRule(rule: ValidationRule) {
  const between = (kind: string, min?: number | string, max?: number | string, low = 'at least', high = 'at most') =>
    min !== undefined && max !== undefined ? `${kind} from ${min} to ${max}` : min !== undefined ? `${kind} ${low} ${min}` : max !== undefined ? `${kind} ${high} ${max}` : kind
  switch (rule.allow) {
    case 'list':
      return `one of ${(rule.choices ?? []).join(', ')}`
    case 'checkbox':
      return 'TRUE or FALSE'
    case 'number':
      return between(rule.whole ? 'a whole number' : 'a number', rule.min, rule.max)
    case 'date':
      return between('a date', rule.from, rule.to, 'on or after', 'on or before')
    case 'textLength':
      return rule.min !== undefined && rule.max !== undefined
        ? `text of ${rule.min} to ${rule.max} characters`
        : rule.min !== undefined
          ? `text of at least ${rule.min} characters`
          : rule.max !== undefined
            ? `text of at most ${rule.max} characters`
            : 'text'
    default:
      return 'anything'
  }
}

/** Why a cell is marked: the rule it breaks, with its hint. */
export function markTitle(rule: ValidationRule) {
  return `Should be ${describeRule(rule)}${rule.hint ? ` (${rule.hint})` : ''}`
}
