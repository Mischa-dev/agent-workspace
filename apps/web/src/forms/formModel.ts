/**
 * A collection's public form (AWD-110), as `/forms/{token}` reads it: the
 * collection's name and the fields the form asks for, nothing else. Answers
 * go to the link's report URL like any bug report, and each one becomes a
 * record, made as the person who shared the form.
 */

export type PublicFieldType = 'text' | 'long_text' | 'number' | 'date' | 'single_choice' | 'multiple_choice' | 'status' | 'url' | 'email' | 'phone' | 'checkbox'

export type PublicField = {
  id: string
  name: string
  description: string
  type: PublicFieldType
  required: boolean
  options: { id: string; name: string; color: string }[]
}

export type PublicForm = {
  name: string
  collection: string
  titleName: string
  intro: string
  thanks: string
  fields: PublicField[]
}

/** What the person typed or picked, by field ID; the title under "title". */
export type Answers = Record<string, string | string[] | boolean>

type Wire = Record<string, unknown>
const asRecord = (value: unknown): Wire => (value && typeof value === 'object' && !Array.isArray(value) ? (value as Wire) : {})
const asString = (value: unknown) => (typeof value === 'string' ? value : '')
const TYPES: readonly PublicFieldType[] = ['text', 'long_text', 'number', 'date', 'single_choice', 'multiple_choice', 'status', 'url', 'email', 'phone', 'checkbox']

export function normalizeForm(value: unknown): PublicForm {
  const raw = asRecord(value)
  return {
    name: asString(raw.name),
    collection: asString(raw.collection),
    titleName: asString(raw.titleName) || 'Title',
    intro: asString(raw.intro),
    thanks: asString(raw.thanks),
    fields: (Array.isArray(raw.fields) ? raw.fields : []).flatMap((entry) => {
      const field = asRecord(entry)
      const type = TYPES.find((candidate) => candidate === field.type)
      if (!type || typeof field.id !== 'string') return []
      return [
        {
          id: field.id,
          name: asString(field.name),
          description: asString(field.description),
          type,
          required: field.required === true,
          options: (Array.isArray(field.options) ? field.options : []).flatMap((option) => {
            const parts = asRecord(option)
            return typeof parts.id === 'string' ? [{ id: parts.id, name: asString(parts.name), color: asString(parts.color) }] : []
          }),
        },
      ]
    }),
  }
}

/** The first thing the person still has to fill in, in words, or null. */
export function missing(form: PublicForm, answers: Answers): string | null {
  const title = answers.title
  if (typeof title !== 'string' || !title.trim()) return `Fill in ${form.titleName}.`
  for (const field of form.fields) {
    if (!field.required || field.type === 'checkbox') continue
    const value = answers[field.id]
    const empty = value === undefined || (typeof value === 'string' && !value.trim()) || (Array.isArray(value) && value.length === 0)
    if (empty) return `Fill in ${field.name}.`
  }
  return null
}

/** The answers as the report URL takes them: by field ID, blanks left out. */
export function answerBody(form: PublicForm, answers: Answers): Record<string, string | string[] | boolean | number> {
  const body: Record<string, string | string[] | boolean | number> = { title: String(answers.title ?? '').trim() }
  for (const field of form.fields) {
    const value = answers[field.id]
    if (value === undefined) continue
    if (typeof value === 'string') {
      if (!value.trim()) continue
      body[field.id] = field.type === 'number' && Number.isFinite(Number(value)) ? Number(value) : value.trim()
    } else if (Array.isArray(value)) {
      if (value.length) body[field.id] = value
    } else {
      body[field.id] = value
    }
  }
  return body
}

/** Reads the form; a closed or missing one says so. */
export async function fetchForm(token: string): Promise<PublicForm> {
  const response = await fetch(`/api/v1/forms/${encodeURIComponent(token)}`, { credentials: 'omit' })
  const body: unknown = await response.json().catch(() => null)
  if (!response.ok) throw new FormProblem(response.status, asString(asRecord(body).error) || 'This form didn’t load.')
  return normalizeForm(body)
}

/** Sends the answers; resolves with the thank-you note. */
export async function sendAnswers(token: string, form: PublicForm, answers: Answers): Promise<string> {
  const response = await fetch(`/api/v1/bug-reports/${encodeURIComponent(token)}`, {
    method: 'POST',
    credentials: 'omit',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(answerBody(form, answers)),
  })
  const body = asRecord(await response.json().catch(() => null))
  if (!response.ok) throw new FormProblem(response.status, asString(body.error) || 'Your answers weren’t sent. Try again.')
  return asString(body.thanks)
}

export class FormProblem extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message)
  }
}
