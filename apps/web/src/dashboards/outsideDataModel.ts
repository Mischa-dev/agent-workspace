import { projectAreaHref } from '../projects/locations'
import { countLabel, formatRelative } from '../ui'

export const outsideDataHref = (projectId: string) => projectAreaHref(projectId, 'dashboards', 'data')
export const dataQueryHref = (projectId: string, queryId: string) => projectAreaHref(projectId, 'dashboards', 'data', { query: queryId })

export type ParameterType = 'text' | 'number' | 'date' | 'boolean'

/** A parameter as the editor holds it: its default as typed. */
export type ParameterDraft = { name: string; type: ParameterType; value: string }

/** The parameter names a query's text uses, `{{name}}`, in order, once each. */
export function usedParameters(statement: string): string[] {
  const names: string[] = []
  for (const match of statement.matchAll(/\{\{\s*([a-z][a-z0-9_]*)\s*\}\}/g)) {
    if (!names.includes(match[1])) names.push(match[1])
  }
  return names
}

/** A typed default as the server takes it: null when empty, a number or a boolean when the type says so. */
export function parameterValue(type: ParameterType, value: string): string | number | boolean | null {
  const trimmed = value.trim()
  if (!trimmed) return null
  if (type === 'number') {
    const number = Number(trimmed)
    return Number.isFinite(number) ? number : trimmed
  }
  if (type === 'boolean') return trimmed === 'true'
  return trimmed
}

/** A stored default as the editor shows it. */
export function parameterText(value: unknown): string {
  if (value === null || value === undefined) return ''
  return typeof value === 'string' ? value : String(value)
}

/** One cell of an answer, as text. */
export function cellText(value: unknown): string {
  if (value === null || value === undefined) return ''
  if (typeof value === 'string') return value
  if (typeof value === 'number' || typeof value === 'boolean') return String(value)
  return JSON.stringify(value)
}

/** What a stored answer says about itself: when it ran, how many rows, and whether some were left out. */
export function answerWords(answer: { ranAt?: string | null; rowCount: number; truncated: boolean; durationMs?: number | null }): string {
  const parts: string[] = []
  if (answer.ranAt) parts.push(`Ran ${formatRelative(answer.ranAt)}`)
  parts.push(answer.truncated ? `the first ${countLabel(answer.rowCount, 'row')} (more came back than the query keeps)` : countLabel(answer.rowCount, 'row'))
  if (typeof answer.durationMs === 'number') parts.push(`${answer.durationMs.toLocaleString()} ms`)
  return parts.join(' · ')
}
